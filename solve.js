// Fast drop-in for make.fun's csv-reader 1.0.0: run(input) returns exactly
// csv-reader's answer {columns, rows, errors} for every input {text, header},
// but reads the text with bulk scans instead of one character at a time.
//
// The policy is csv-reader's own, copied from its reference (baselines/
// csv-reader/vendor/csv-reader/index.js in the kit):
//   - a U+FEFF at the very start of the text is dropped;
//   - a record ends at LF or CRLF, and a line left with no characters is no record;
//   - outside a quoted field a CR not followed by LF is dropped (stray_cr);
//   - a field that opens with a quote runs to the next quote that is not doubled,
//     "" inside it one quote, and each CRLF or lone CR inside it read as LF;
//     what follows its closing quote, up to the next comma or line break, is
//     appended as it stands (quote_junk, when the field ends); a quote that
//     never closes is a literal character, and the text after it is read again
//     as unquoted (unterminated_quote);
//   - a quote inside an unquoted field is a literal character; fields stay
//     strings, never trimmed or converted;
//   - columns is the first record when header is true, else null; rows holds
//     the records after it, each padded with "" (short_row, with fields: its
//     count) or cut (long_row, with extra: the cut fields) to the first
//     record's width;
//   - each error is {code, line, row}, in the order met; line the 1-based line
//     its record starts on, row its record's index in rows (null for the header).
//
// Speed comes from three choices the reference does not make: an unquoted field
// is found with one regex scan for the next delimiter instead of a per-char
// loop; a quoted field is found with indexOf for its closing quote and sliced
// whole instead of built a character at a time; and a field is only copied
// when it has to be (a stray CR, a doubled quote, or a line break inside
// quotes). Pure: no clock, no randomness, no locale, nothing kept from one
// call to the next.

/** How a field ended: 0 at a comma, 1 at a line break, 2 at the end of the text. */
const COMMA = 0;
const LINE_BREAK = 1;
const END = 2;

/** The reading of one CSV input, {text, header}: {columns, rows, errors}. */
export function run(input) {
  const text = String(input.text);
  const header = input.header === true;
  const errors = [];
  const rows = [];
  let columns = null;
  let width = -1;

  const n = text.length;
  let pos = text.charCodeAt(0) === 0xfeff ? 1 : 0; // the next character to read
  let line = 1; // the line that character is on

  // The record being read: the line it starts on and the index in rows it takes.
  let recordLine = 1;
  let recordRow = null;

  function error(code) {
    errors.push({ code, line: recordLine, row: recordRow });
  }

  /** The next comma, LF or CR at or after a position, in one scan. */
  const DELIM = /[,\n\r]/g;

  // Reads unquoted characters onto `init` up to the next comma or line break:
  // a CR not followed by LF is dropped (stray_cr). Returns the field, how it
  // ended, and how many characters were kept onto `init`.
  function readUnquoted(init) {
    let acc = null; // kept parts, init included, once a stray CR forced a copy
    let start = pos;
    while (true) {
      DELIM.lastIndex = start;
      const m = DELIM.exec(text);
      if (m === null) {
        const value = acc === null ? init + text.slice(start) : acc + text.slice(start);
        pos = n;
        return { value, ended: END, added: value.length - init.length };
      }
      const d = m.index;
      const c = text.charCodeAt(d);
      if (c === 44) {
        const value = acc === null ? init + text.slice(start, d) : acc + text.slice(start, d);
        pos = d + 1;
        return { value, ended: COMMA, added: value.length - init.length };
      }
      if (c === 10) {
        const value = acc === null ? init + text.slice(start, d) : acc + text.slice(start, d);
        pos = d + 1;
        line += 1;
        return { value, ended: LINE_BREAK, added: value.length - init.length };
      }
      // A CR: a line break when followed by LF, else a stray CR that is dropped
      // and the field goes on after it.
      if (text.charCodeAt(d + 1) === 10) {
        const value = acc === null ? init + text.slice(start, d) : acc + text.slice(start, d);
        pos = d + 2;
        line += 1;
        return { value, ended: LINE_BREAK, added: value.length - init.length };
      }
      error('stray_cr');
      acc = (acc === null ? init : acc) + text.slice(start, d);
      start = d + 1;
    }
  }

  // Reads a quoted field whose opening quote is at pos.
  function readQuoted() {
    const open = pos;
    const openLine = line;
    let acc = null; // kept parts, one literal quote per doubled pair, once needed
    let segStart = pos + 1;
    let scan = pos + 1;
    let closedAt = -1;
    while (true) {
      const q = text.indexOf('"', scan);
      if (q === -1) {
        // The quote never closes: it is a literal character, and the text after
        // it is read again as unquoted.
        pos = open + 1;
        line = openLine;
        error('unterminated_quote');
        return readUnquoted('"');
      }
      if (text.charCodeAt(q + 1) === 34) {
        acc = (acc === null ? '' : acc) + text.slice(segStart, q) + '"';
        segStart = q + 2;
        scan = q + 2;
        continue;
      }
      closedAt = q;
      break;
    }
    let value = acc === null ? text.slice(segStart, closedAt) : acc + text.slice(segStart, closedAt);
    // CRLF and a lone CR inside quotes are each one LF, and each counts a line:
    // one scan normalises them and counts them together.
    if (value.indexOf('\n') !== -1 || value.indexOf('\r') !== -1) {
      value = value.replace(/\r\n?|\n/g, () => {
        line += 1;
        return '\n';
      });
    }
    pos = closedAt + 1;
    if (pos >= n) return { value, ended: END };
    const c = text.charCodeAt(pos);
    if (c === 44) {
      pos += 1;
      return { value, ended: COMMA };
    }
    if (c === 10) {
      pos += 1;
      line += 1;
      return { value, ended: LINE_BREAK };
    }
    if (c === 13 && text.charCodeAt(pos + 1) === 10) {
      pos += 2;
      line += 1;
      return { value, ended: LINE_BREAK };
    }
    // What follows the closing quote is appended as it stands, up to the next
    // comma or line break.
    const junk = readUnquoted(value);
    if (junk.added > 0) error('quote_junk');
    return junk;
  }

  while (true) {
    // Lines with no characters are no records.
    while (pos < n) {
      if (text.charCodeAt(pos) === 10) {
        pos += 1;
        line += 1;
      } else if (text.charCodeAt(pos) === 13 && text.charCodeAt(pos + 1) === 10) {
        pos += 2;
        line += 1;
      } else {
        break;
      }
    }
    if (pos >= n) break;

    recordLine = line;
    recordRow = header && columns === null ? null : rows.length;
    const fields = [];
    let field;
    let quoted;
    do {
      quoted = text.charCodeAt(pos) === 34;
      field = quoted ? readQuoted() : readUnquoted('');
      fields.push(field.value);
    } while (field.ended === COMMA);

    // A line that held only stray CRs is left with no characters: no record
    // (its stray_cr errors stay).
    if (fields.length === 1 && fields[0] === '' && !quoted) continue;

    if (width < 0) width = fields.length;
    if (header && columns === null) {
      columns = fields;
      continue;
    }
    if (fields.length < width) {
      errors.push({ code: 'short_row', line: recordLine, row: recordRow, fields: fields.length });
      while (fields.length < width) fields.push('');
    } else if (fields.length > width) {
      errors.push({ code: 'long_row', line: recordLine, row: recordRow, extra: fields.splice(width) });
    }
    rows.push(fields);
  }
  return { columns: header ? (columns ?? []) : null, rows, errors };
}

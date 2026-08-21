/**
 * A TOML colourizer for the landing page's code samples.
 *
 * The doc pages get Shiki through markdown, but the landing page's samples
 * live inside Vue components, where the markdown pipeline never reaches them.
 * Shipping Shiki to the client to highlight five constant strings costs more
 * than the strings weigh, so this walks them instead: TOML is small enough
 * that a scanner is a screenful, and the classes it emits are styled in
 * `landing.css` beside everything else.
 *
 * The output is inserted with `v-html`, which is safe here and nowhere else -
 * every input is a constant written in this repository, and the escaping below
 * is what keeps it that way if one ever grows a `<`.
 */

const ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
};

const escape = (text: string) => text.replace(/[&<>]/g, (c) => ESCAPES[c]);

const span = (cls: string, text: string) =>
  `<span class="${cls}">${escape(text)}</span>`;

/** A line that is only a table header: `[theme]`, `[[hooks]]`, `[pane.alert]`. */
const TABLE = /^(\s*)(\[\[?[A-Za-z0-9_.-]+\]?\])(.*)$/;

/**
 * Strings first, so a `#` or a bracket inside one is never read as syntax.
 * Then literals, then a bare word that some `=` is about to be assigned to,
 * then the punctuation that holds an inline table together.
 */
const TOKEN =
  /("(?:[^"\\]|\\.)*"|'[^']*')|\b(true|false)\b|(-?\d+(?:\.\d+)?)|([A-Za-z_][A-Za-z0-9_-]*)(?=\s*=)|([[\]{},=])/g;

/** The index of the `#` that opens a comment, or -1. Ignores one in a string. */
function commentAt(line: string): number {
  let quote = "";
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quote) {
      if (ch === "\\") i++;
      else if (ch === quote) quote = "";
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === "#") {
      return i;
    }
  }
  return -1;
}

function values(code: string): string {
  let out = "";
  let last = 0;
  for (const m of code.matchAll(TOKEN)) {
    const [text, string, bool, number, key] = m;
    out += escape(code.slice(last, m.index));
    out += string
      ? span("tk-str", text)
      : bool
        ? span("tk-bool", text)
        : number
          ? span("tk-num", text)
          : key
            ? span("tk-key", text)
            : span("tk-punct", text);
    last = m.index + text.length;
  }
  return out + escape(code.slice(last));
}

function line(source: string): string {
  const comment = commentAt(source);
  const code = comment === -1 ? source : source.slice(0, comment);
  const trailing =
    comment === -1 ? "" : span("tk-comment", source.slice(comment));

  const table = TABLE.exec(code);
  if (table) {
    return table[1] + span("tk-table", table[2]) + values(table[3]) + trailing;
  }
  return values(code) + trailing;
}

export const highlightToml = (source: string): string =>
  source.split("\n").map(line).join("\n");

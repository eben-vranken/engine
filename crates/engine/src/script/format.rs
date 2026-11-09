//! Reading a script as tokens (for colors in the code editor) and
//! formatting it (on save): four-space indents by nesting, one space
//! around operators and after commas, `{` on the line it opens, at most
//! one blank line in a row.
//!
//! Formatting only moves whitespace. It is checked: if the formatted text
//! doesn't read as exactly the same tokens, the text is left as it was.

/// What a piece of a script is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    Ident,
    Number,
    Text,
    Comment,
    Punct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    /// Byte range in the text.
    pub start: usize,
    pub end: usize,
}

const KEYWORDS: &[&str] = &[
    "let",
    "const",
    "fn",
    "if",
    "else",
    "while",
    "loop",
    "for",
    "in",
    "do",
    "until",
    "break",
    "continue",
    "return",
    "throw",
    "try",
    "catch",
    "switch",
    "true",
    "false",
    "this",
    "private",
    "import",
    "export",
    "as",
    "global",
    "is_def_var",
    "Fn",
    "call",
    "curry",
];

/// Punctuation, longest first.
const PUNCT: &[&str] = &[
    "..=", "<<=", ">>=", "**=", "::", "..", "=>", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=",
    "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "**", "?.", "?[", "??", "#{",
];

/// Splits `text` into tokens; whitespace is what lies between them.
pub fn tokens(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let rest = &text[i..];
        let kind = if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
            Kind::Comment
        } else if rest.starts_with("/*") {
            // Rhai's block comments nest.
            let mut depth = 0;
            while i < bytes.len() {
                if text[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if text[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            Kind::Comment
        } else if c == b'"' || c == b'`' || c == b'\'' {
            i += 1;
            while i < bytes.len() && bytes[i] != c {
                if bytes[i] == b'\\' && c != b'`' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            Kind::Text
        } else if c.is_ascii_digit() {
            while i < bytes.len() {
                let d = bytes[i];
                let decimal_point = d == b'.'
                    && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
                    && !text[start..i].contains('.');
                let exponent_sign = (d == b'-' || d == b'+')
                    && matches!(bytes[i - 1], b'e' | b'E')
                    && !text[start..i].starts_with("0x");
                if d.is_ascii_alphanumeric() || d == b'_' || decimal_point || exponent_sign {
                    i += 1;
                } else {
                    break;
                }
            }
            Kind::Number
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < bytes.len() {
                let d = bytes[i];
                if d.is_ascii_alphanumeric() || d == b'_' || d >= 0x80 {
                    i += 1;
                } else {
                    break;
                }
            }
            if KEYWORDS.contains(&&text[start..i]) {
                Kind::Keyword
            } else {
                Kind::Ident
            }
        } else {
            i += PUNCT
                .iter()
                .find(|p| rest.starts_with(**p))
                .map_or(1, |p| p.len());
            Kind::Punct
        };
        // Never split a character in two.
        while !text.is_char_boundary(i) {
            i += 1;
        }
        out.push(Token {
            kind,
            start,
            end: i,
        });
    }
    out
}

/// Operators that get one space on each side.
const BINARY: &[&str] = &[
    "=", "==", "!=", "<", ">", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "+=", "-=", "*=",
    "/=", "%=", "&=", "|=", "^=", "<<", ">>", "**", "<<=", ">>=", "**=", "=>", "??", "&", "^",
    "in",
];

/// The script formatted, or `text` unchanged if formatting would change
/// what it says.
pub fn format(text: &str) -> String {
    let formatted = reprint(text);
    let words = |t: &str| -> Vec<String> {
        tokens(t)
            .iter()
            .map(|k| t[k.start..k.end].to_owned())
            .collect()
    };
    if words(&formatted) == words(text) {
        formatted
    } else {
        text.to_owned()
    }
}

/// For each `{` token: whether its block holds statements (a `;`
/// anywhere inside), which puts each of them on a line of its own.
fn statement_blocks(text: &str, tokens: &[Token]) -> Vec<bool> {
    let mut expanded = vec![false; tokens.len()];
    let mut open: Vec<usize> = Vec::new();
    for (n, token) in tokens.iter().enumerate() {
        match &text[token.start..token.end] {
            "{" | "(" | "[" | "#{" | "?[" => open.push(n),
            "}" | ")" | "]" => {
                open.pop();
            }
            ";" => {
                for &o in &open {
                    if &text[tokens[o].start..tokens[o].end] == "{" {
                        expanded[o] = true;
                    }
                }
            }
            _ => {}
        }
    }
    expanded
}

fn is_opener(word: &str) -> bool {
    matches!(word, "{" | "(" | "[" | "#{" | "?[")
}

fn reprint(text: &str) -> String {
    let tokens = tokens(text);
    let expanded = statement_blocks(text, &tokens);
    let mut out = String::new();
    let mut depth: usize = 0;
    // The open brackets: whether each is a block of statements.
    let mut open: Vec<bool> = Vec::new();
    // A line break is due before the next token.
    let mut break_next = false;
    let mut prev: Option<(Token, &str)> = None;
    for (n, token) in tokens.iter().enumerate() {
        let word = &text[token.start..token.end];
        let gap = &text[prev.map_or(0, |(p, _)| p.end)..token.start];
        let mut newlines = gap.matches('\n').count();
        let closes = matches!(word, "}" | ")" | "]");
        let mut closes_block = false;
        if closes {
            depth = depth.saturating_sub(1);
            closes_block = open.pop() == Some(true);
        }
        // A comment at the end of a line stays there; `} else`, `};`,
        // `},` follow their block on its line.
        let trailing_comment = token.kind == Kind::Comment && newlines == 0;
        let follows_block = prev.is_some_and(|(_, p)| p == "}")
            && matches!(word, "else" | "catch" | ";" | "," | ")" | "]" | ".");
        if (closes_block || (break_next && !follows_block)) && !trailing_comment && newlines == 0 {
            newlines = 1;
        }
        match prev {
            None => {}
            Some(_) if newlines > 0 => {
                out.push('\n');
                if newlines > 1 {
                    out.push('\n');
                }
                out.push_str(&"    ".repeat(depth));
            }
            Some((p, pword)) if space_between(p, pword, *token, word, &tokens[..n], text, gap) => {
                out.push(' ');
            }
            Some(_) => {}
        }
        out.push_str(word);
        // Breaks after a block of statements opens, after a statement in
        // one (or at the top), and after such a block closes.
        break_next = if is_opener(word) {
            depth += 1;
            open.push(expanded[n]);
            expanded[n]
        } else if word == ";" {
            open.last() != Some(&false)
        } else if token.kind == Kind::Comment {
            break_next
        } else {
            closes_block
        };
        prev = Some((*token, word));
    }
    // No spaces at line ends, one newline at the end.
    let mut lines: Vec<&str> = out.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut result = lines.join("\n");
    result.push('\n');
    result
}

/// Whether `prev` is a value an operator after it applies to.
fn is_value(kind: Kind, word: &str) -> bool {
    match kind {
        Kind::Ident | Kind::Number | Kind::Text => true,
        Kind::Keyword => matches!(word, "true" | "false" | "this"),
        Kind::Punct => matches!(word, ")" | "]" | "}"),
        Kind::Comment => false,
    }
}

/// Whether one space goes between two tokens on the same line.
fn space_between(
    prev: Token,
    pword: &str,
    token: Token,
    word: &str,
    before: &[Token],
    text: &str,
    gap: &str,
) -> bool {
    let had_space = !gap.is_empty();
    // The token before `prev`, to tell `a - b` from `x = -b`.
    let unary = |op: Token, opword: &str| -> bool {
        matches!(opword, "-" | "+" | "!")
            && before
                .iter()
                .rev()
                .find(|t| t.start < op.start)
                .is_none_or(|t| !is_value(t.kind, &text[t.start..t.end]))
    };
    if token.kind == Kind::Comment {
        return true;
    }
    if prev.kind == Kind::Comment {
        return had_space;
    }
    // Never glued: `,` `;` `.` `::` `?.` and closers after; openers before.
    if matches!(word, "," | ";" | "." | "::" | "?." | ")" | "]") {
        return false;
    }
    if matches!(pword, "(" | "[" | "." | "::" | "?." | "?[") {
        return false;
    }
    if matches!(pword, "," | ";" | "}") {
        return true;
    }
    if pword == "!" || (matches!(pword, "-" | "+") && unary(prev, pword)) {
        return false;
    }
    if word == ":" {
        return false;
    }
    if pword == ":" {
        return true;
    }
    if word == "{" || word == "#{" {
        return true;
    }
    if pword == "{" || pword == "#{" {
        return word != "}";
    }
    if word == "}" {
        return pword != "{" && pword != "#{";
    }
    if word == "(" || word == "[" || word == "?[" {
        // A call or an index right after a name; a space after keywords.
        return match prev.kind {
            Kind::Keyword => !matches!(pword, "this" | "Fn" | "call" | "curry" | "fn"),
            Kind::Punct => !matches!(pword, ")" | "]") && had_space,
            _ => false,
        };
    }
    if BINARY.contains(&word) {
        // Binary: spaced. Unary (`x = -1`, `return -1`): spaced from what's
        // before it (openers were dealt with above), glued to its value.
        return true;
    }
    if BINARY.contains(&pword) {
        return true;
    }
    if matches!(prev.kind, Kind::Punct) || matches!(token.kind, Kind::Punct) {
        // Closures (`|x| x + 1`), ranges and anything else: as written.
        return had_space;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indents_and_spaces_are_set() {
        let text = "fn update(player,dt){\nif player.grounded{this.used=0;}\n  let x=-1+2*3;\n\n\n\tplayer.velocity.y=x;\n}";
        assert_eq!(
            format(text),
            "fn update(player, dt) {\n    if player.grounded {
        this.used = 0;
    }\n    let x = -1 + 2 * 3;\n\n    player.velocity.y = x;\n}\n"
        );
    }

    #[test]
    fn strings_and_comments_stay_as_written() {
        let text = "let s = \"a  ,b{\"; // keep   this\n/* and\n   this */\nprint(`x ${s}`);\n";
        assert_eq!(format(text), text);
    }

    #[test]
    fn already_formatted_text_is_unchanged() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sandbox/scripts");
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            let text = text.replace("\r\n", "\n");
            assert_eq!(format(&text), text, "{}", entry.path().display());
        }
    }

    #[test]
    fn formatting_never_changes_the_tokens() {
        for text in [
            "let a = b - -c;",
            "let f = |x| x+1; let r = 1..=5; let m = #{a:1, b:[1,2]};",
            "if !done && x > -2.5e-3 { return; } else if a?.b ?? 0 { }",
            "switch x { 1 => print(\"one\"), _ => () }",
            "let v = 1.0; let w = 0x1F; let t = 'c'; x::y();",
        ] {
            let words = |t: &str| -> Vec<String> {
                tokens(t)
                    .iter()
                    .map(|k| t[k.start..k.end].to_owned())
                    .collect()
            };
            assert_eq!(words(&reprint(text)), words(text), "{text}");
        }
        assert_eq!(
            format("let m = #{a:1, b:[1,2]};"),
            "let m = #{ a: 1, b: [1, 2] };\n"
        );
        assert_eq!(format("let a = b - -c;"), "let a = b - -c;\n");
    }

    #[test]
    fn statements_go_on_lines_of_their_own() {
        assert_eq!(
            format("fn dash(player,dt){let a=-1+2*3;if a>1{print(a);} else {a=0;}} // done"),
            "fn dash(player, dt) {
    let a = -1 + 2 * 3;
    if a > 1 {
        print(a);
    } else {
        a = 0;
    }
} // done
"
        );
        // Value blocks and maps stay on one line.
        assert_eq!(
            format("let m = #{a: 1}; let b = if x { 1 } else { 2 };"),
            "let m = #{ a: 1 };
let b = if x { 1 } else { 2 };
"
        );
    }

    #[test]
    fn tokens_cover_numbers_and_operators() {
        let text = "x.y+=1.5;";
        let words: Vec<&str> = tokens(text).iter().map(|t| &text[t.start..t.end]).collect();
        assert_eq!(words, ["x", ".", "y", "+=", "1.5", ";"]);
    }
}

//! TextMate scope selectors, used to decide where injection grammars apply.
//!
//! A selector is a comma/`|`-separated list of groups, each optionally prefixed with a
//! priority (`L:` before, `R:` after, `B:`). Within a group, space-separated scope tokens
//! match as an ordered (descendant) subsequence of the scope stack, `-` negates, and
//! parentheses group. A scope token matches a scope when it is a segment-aligned prefix
//! of it (`meta.tag` matches `meta.tag.html`), with `*` matching any single segment.
//!
//! This mirrors the matcher in `vscode-textmate`.

/// A parsed scope selector: a set of `(priority, expression)` alternatives.
#[derive(Debug, Clone)]
pub struct Selector {
    groups: Vec<(i8, Node)>,
}

#[derive(Debug, Clone)]
enum Node {
    /// Space-separated scope tokens, matched as an ordered subsequence.
    Path(Vec<String>),
    Neg(Box<Node>),
    And(Vec<Node>),
    Or(Vec<Node>),
}

#[derive(Debug, PartialEq)]
enum Tok {
    Prefix(i8),
    Ident(String),
    Comma,
    Pipe,
    Minus,
    Open,
    Close,
}

fn tokenize(s: &str) -> Vec<Tok> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'*' | b'_' | b'-');
    let is_ident_start = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'*' | b'_');
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Priority prefix: `L:` / `R:` / `B:`.
        if i + 1 < bytes.len() && bytes[i + 1] == b':' && matches!(c, b'L' | b'R' | b'B') {
            let p = match c {
                b'L' => 1,  // before base patterns -> higher priority
                b'R' => -1, // after base patterns -> lower priority
                _ => 0,
            };
            out.push(Tok::Prefix(p));
            i += 2;
            continue;
        }
        match c {
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            b'|' => {
                out.push(Tok::Pipe);
                i += 1;
            }
            b'-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            b'(' => {
                out.push(Tok::Open);
                i += 1;
            }
            b')' => {
                out.push(Tok::Close);
                i += 1;
            }
            _ if is_ident_start(c) => {
                let start = i;
                while i < bytes.len() && is_ident(bytes[i]) {
                    i += 1;
                }
                out.push(Tok::Ident(s[start..i].to_string()));
            }
            _ => i += 1, // skip anything unexpected
        }
    }
    out
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn parse_operand(&mut self) -> Option<Node> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.pos += 1;
                let inner = self.parse_operand()?;
                Some(Node::Neg(Box::new(inner)))
            }
            Some(Tok::Open) => {
                self.pos += 1;
                let inner = self.parse_or();
                if matches!(self.peek(), Some(Tok::Close)) {
                    self.pos += 1;
                }
                Some(inner)
            }
            Some(Tok::Ident(_)) => {
                let mut idents = Vec::new();
                while let Some(Tok::Ident(s)) = self.peek() {
                    idents.push(s.clone());
                    self.pos += 1;
                }
                Some(Node::Path(idents))
            }
            _ => None,
        }
    }

    /// Conjunction: space-separated operands, all of which must match.
    fn parse_and(&mut self) -> Option<Node> {
        let mut operands = Vec::new();
        while let Some(op) = self.parse_operand() {
            operands.push(op);
        }
        match operands.len() {
            0 => None,
            1 => operands.pop(),
            _ => Some(Node::And(operands)),
        }
    }

    /// Inner expression: conjunctions separated by `,` or `|`, any of which may match.
    fn parse_or(&mut self) -> Node {
        let mut alts = Vec::new();
        if let Some(node) = self.parse_and() {
            alts.push(node);
        }
        while matches!(self.peek(), Some(Tok::Comma | Tok::Pipe)) {
            while matches!(self.peek(), Some(Tok::Comma | Tok::Pipe)) {
                self.pos += 1;
            }
            if let Some(node) = self.parse_and() {
                alts.push(node);
            }
        }
        if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Node::Or(alts)
        }
    }
}

impl Selector {
    /// Parses a scope selector. Always succeeds (an unparseable selector simply never
    /// matches).
    pub fn parse(s: &str) -> Self {
        let mut parser = Parser {
            toks: tokenize(s),
            pos: 0,
        };
        let mut groups = Vec::new();
        while parser.peek().is_some() {
            let priority = if let Some(Tok::Prefix(p)) = parser.peek() {
                let p = *p;
                parser.pos += 1;
                p
            } else {
                0
            };
            // Each top-level comma group is its own alternative; `parse_and` stops at a
            // separator. Consume one conjunction, then a separating comma if present.
            if let Some(node) = parser.parse_and() {
                groups.push((priority, node));
            }
            if matches!(parser.peek(), Some(Tok::Comma | Tok::Pipe)) {
                parser.pos += 1;
            } else if parser.peek().is_some() && !matches!(parser.peek(), Some(Tok::Prefix(_))) {
                // Avoid an infinite loop on unexpected tokens.
                parser.pos += 1;
            }
        }
        Selector { groups }
    }

    /// Returns the priority of the first group that matches `scopes` (outermost first),
    /// or `None` if the selector does not match.
    pub fn matches(&self, scopes: &[String]) -> Option<i8> {
        self.groups
            .iter()
            .find(|(_, node)| node.matches(scopes))
            .map(|(p, _)| *p)
    }
}

impl Node {
    fn matches(&self, scopes: &[String]) -> bool {
        match self {
            Node::Path(ids) => path_matches(ids, scopes),
            Node::Neg(inner) => !inner.matches(scopes),
            Node::And(v) => v.iter().all(|n| n.matches(scopes)),
            Node::Or(v) => v.iter().any(|n| n.matches(scopes)),
        }
    }
}

/// Ordered-subsequence match: each token must match some scope at or after the previous
/// match's position.
fn path_matches(ids: &[String], scopes: &[String]) -> bool {
    let mut last = 0;
    for id in ids {
        let mut found = false;
        for (i, scope) in scopes.iter().enumerate().skip(last) {
            if scope_token_matches(scope, id) {
                last = i + 1;
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}

/// A token matches a scope if it is a segment-aligned prefix of it, with `*` matching any
/// single segment (`meta.tag` and `meta.*` both match `meta.tag.html`).
fn scope_token_matches(scope: &str, token: &str) -> bool {
    let mut s = scope.split('.');
    for t in token.split('.') {
        match s.next() {
            Some(seg) if t == "*" || t == seg => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn prefix_match() {
        assert!(scope_token_matches("meta.tag.html", "meta.tag"));
        assert!(scope_token_matches("meta.tag.html", "meta.*.html"));
        assert!(!scope_token_matches("meta.tag.html", "meta.tag.html.x"));
        assert!(!scope_token_matches("metaphor.x", "meta"));
    }

    #[test]
    fn descendant_and_priority() {
        let sel = Selector::parse("L:text.html meta.embedded");
        assert_eq!(
            sel.matches(&scopes("text.html.basic meta.embedded.block.html source.css")),
            Some(1)
        );
        assert_eq!(sel.matches(&scopes("text.html.basic")), None);
    }

    #[test]
    fn negation_and_groups() {
        let sel = Selector::parse("L:text.html -comment");
        assert_eq!(sel.matches(&scopes("text.html.basic")), Some(1));
        assert_eq!(sel.matches(&scopes("text.html.basic comment.line")), None);

        let sel = Selector::parse("source.js - (comment, string)");
        assert!(sel.matches(&scopes("source.js")).is_some());
        assert!(sel.matches(&scopes("source.js string.quoted")).is_none());
        assert!(sel.matches(&scopes("source.js comment.line")).is_none());
    }

    #[test]
    fn alternatives() {
        let sel = Selector::parse("source.js, source.ts");
        assert!(sel.matches(&scopes("source.ts")).is_some());
        assert!(sel.matches(&scopes("source.js")).is_some());
        assert!(sel.matches(&scopes("source.rust")).is_none());
    }
}

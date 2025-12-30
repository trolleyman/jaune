use crate::{
    Scope,
    syntax::{Pattern, SyntaxDefinition},
};
use fancy_regex::Regex;

/// Operations emitted by the [`Tokenizer`].
///
/// These variants describe the stream of parsing events. Consumers should interpret these
/// to construct the final list of tokens or syntax-highlighted regions.
///
/// # Handling State
/// The [`Tokenizer`] does not maintain the "scope stack" (the hierarchy of scopes like
/// `source.rust` -> `meta.function`). Instead, it emits [`Push`](TokenizerOp::Push) and
/// [`Pop`](TokenizerOp::Pop) operations. The consumer is responsible for maintaining a
/// `Vec<Scope>` if they need to know the full context of a [`Content`](TokenizerOp::Content) token.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenizerOp<'a> {
    /// Indicates the start of a new scope.
    ///
    /// The consumer should push this [`Scope`] onto their active stack.
    Push(Scope),

    /// Indicates the end of the most recently pushed scope.
    ///
    /// The consumer should pop the top element from their active stack.
    Pop,

    /// A chunk of text content.
    ///
    /// This text belongs to the context defined by the current state of the consumer's
    /// scope stack (after processing all preceding `Push`/`Pop` operations).
    Content(&'a str),

    /// An explicit newline character.
    ///
    /// This is emitted separately to ensure that regex anchors (like `^` and `$`)
    /// are handled correctly by the parser. It should be treated as a newline in the output.
    Newline,
}

/// Internal state for a nested block (e.g., inside a string or comment).
struct StackFrame {
    /// The regex that will close this block.
    ///
    /// This is compiled dynamically because it may depend on captures from the
    /// opening match (e.g., heredocs like `<<-EOF ... EOF`).
    end_regex: Regex,

    /// The patterns that are valid inside this block.
    patterns: Vec<Pattern>,
}

/// A line-based iterator that parses text according to a [`SyntaxDefinition`].
///
/// This struct manages the internal parsing state (the "grammar stack") but delegates
/// the management of the "scope stack" to the consumer via [`TokenizerOp`]s.
pub struct Tokenizer<'a> {
    text: &'a str,
    cursor: usize,

    /// The stack of grammar rules currently being processed.
    ///
    /// *Note:* This tracks the internal parsing state (which rules are valid),
    /// not the semantic scope stack used for highlighting.
    stack: Vec<StackFrame>,

    syntax: &'a SyntaxDefinition,
}

impl<'a> Tokenizer<'a> {
    /// Creates a new tokenizer for the given text and syntax definition.
    pub fn new(text: &'a str, syntax: &'a SyntaxDefinition) -> Self {
        Self {
            text,
            cursor: 0,
            stack: Vec::new(),
            syntax,
        }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = TokenizerOp<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.text.len() {
            return None;
        }

        // Logic (Simplified):
        // 1. Check if we match the 'end_regex' of the top StackFrame.
        //    If yes -> Pop StackFrame, yield TokenizerOp::Pop.

        // 2. If not, check 'patterns' of the top StackFrame.
        //    If 'Match' found -> yield Push, yield Content, yield Pop.
        //    If 'Begin' found -> push new StackFrame, yield Push, yield Content (for the begin marker).

        // 3. If nothing matches -> Consume 1 char as Content, advance cursor.

        // Placeholder implementation for API demonstration:
        let next_char = &self.text[self.cursor..self.cursor + 1];
        self.cursor += 1;
        Some(TokenizerOp::Content(next_char))
    }
}

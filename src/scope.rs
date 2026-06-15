use std::{
    fmt::{Debug, Display},
    num::NonZeroU16,
    ops::{Deref, DerefMut},
    str::FromStr,
};

use crate::{Atom, atom::AtomParseError};

/// The maximum number of [`Atom`]s a single [`Scope`] can hold.
///
/// Real-world TextMate grammars use scopes with as many as ~12 atoms (e.g.
/// `punctuation.section.arguments.begin.bracket.round.function.member.c`), so this is
/// sized with some headroom above that.
pub const MAX_ATOMS: usize = 16;

/// Error type for [`Scope`] creation.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeParseError {
    /// More than [`MAX_ATOMS`] [`Atom`]s were provided.
    #[error("scope can only hold up to {} atoms", MAX_ATOMS)]
    TooManyAtoms,
    /// Maximum number of unique [`Atom`]s has been reached.
    ///
    /// Currently this is `u16::MAX - 2`.
    ///
    /// See [`AtomRepository`](super::AtomRepository) for more information.
    #[error("maximum number of unique atoms reached")]
    MaxAtomsReached,
}

/// A bit-packed representation of a single [`Scope`] (e.g., `meta.function.rust`).
///
/// Stores up to [`MAX_ATOMS`] [`Atom`]s, using 2 bytes per [`Atom`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scope {
    /// The atoms in the scope. `None` indicates the end of the scope.
    atoms: [Option<Atom>; MAX_ATOMS],
}

impl Scope {
    /// Creates a new [`Scope`] from a string.
    ///
    /// Conventionally scopes are dot-separated lowercase atoms (e.g., `meta.function.rust`).
    /// Lowercase is not enforced, but may lead to unexpected behavior if not followed.
    ///
    /// Whitespace and `.` characters are stripped from the start and end of the string, and consecutive `.` characters are treated as a single separator.
    ///
    /// # Examples
    /// ```
    /// # use jaune::{Scope, Atom};
    /// assert_eq!(Scope::new("meta.function.rust").unwrap().to_string(), "meta.function.rust");
    /// assert_eq!(Scope::new(".meta..function.rust.").unwrap().to_string(), "meta.function.rust");
    /// assert_eq!(Scope::new("   .meta..function.rust.  . . ..").unwrap().to_string(), "meta.function.rust");
    /// assert_eq!(Scope::new(" ..  . . ..").unwrap().to_string(), "");
    /// ```
    ///
    /// # Errors
    /// - [`ScopeParseError::TooManyAtoms`] if more than [`MAX_ATOMS`] [`Atom`]s are provided.
    /// - [`ScopeParseError::MaxAtomsReached`] if any [`Atom`] fails to be created due to reaching the maximum number of unique [`Atom`]s.
    pub fn new(s: &str) -> Result<Self, ScopeParseError> {
        let atoms: Result<Vec<Atom>, AtomParseError> = s
            .trim_matches(|c: char| c.is_whitespace() || c == '.')
            .split('.')
            .filter(|s| s.len() != 0)
            .map(Atom::new)
            .collect();
        match atoms {
            Err(AtomParseError::MaxAtomsReached) => Err(ScopeParseError::MaxAtomsReached),
            Err(AtomParseError::InvalidDotCharacter | AtomParseError::EmptyString) => {
                panic!("other AtomParseErrors should be impossible due to filtering")
            }
            Ok(atoms) => {
                if atoms.len() > MAX_ATOMS {
                    return Err(ScopeParseError::TooManyAtoms);
                }
                Scope::from_slice(&atoms)
            }
        }
    }

    /// Creates a [`Scope`] from a string, never failing on atom count: a scope with more
    /// than [`MAX_ATOMS`] segments is truncated to the first [`MAX_ATOMS`], and segments
    /// that exceed the global atom limit are dropped.
    ///
    /// Used when loading grammars, where a single over-long scope shouldn't sink the
    /// whole grammar.
    pub fn new_lossy(s: &str) -> Self {
        let atoms: Vec<Atom> = s
            .trim_matches(|c: char| c.is_whitespace() || c == '.')
            .split('.')
            .filter(|s| !s.is_empty())
            .filter_map(|s| Atom::new(s).ok())
            .take(MAX_ATOMS)
            .collect();
        Scope::from_slice(&atoms).unwrap_or(Scope {
            atoms: [None; MAX_ATOMS],
        })
    }

    /// Creates a new [`Scope`] from a slice of [`Atom`]s.
    ///
    /// # Examples
    /// ```
    /// # use jaune::{Scope, Atom};
    /// assert_eq!(Scope::from_slice(&[Atom::new("meta").unwrap(), Atom::new("function").unwrap(), Atom::new("rust").unwrap()]).unwrap().to_string(), "meta.function.rust");
    /// assert_eq!(Scope::from_slice(&[Atom::new(" meta ").unwrap(), Atom::new(" function ").unwrap(), Atom::new(" rust ").unwrap()]).unwrap().to_string(), "meta.function.rust");
    /// ```
    ///
    /// # Errors
    /// Returns `ScopeParseError::TooManyAtoms` if more than [`MAX_ATOMS`] atoms are provided.
    pub fn from_slice(atoms: &[Atom]) -> Result<Self, ScopeParseError> {
        if atoms.len() > MAX_ATOMS {
            return Err(ScopeParseError::TooManyAtoms);
        }
        let mut arr = [None; MAX_ATOMS];
        for (i, &atom) in atoms.iter().enumerate() {
            arr[i] = Some(atom);
        }
        Ok(Scope { atoms: arr })
    }

    /// Returns the atoms as a slice.
    pub fn as_slice(&self) -> &[Atom] {
        static_assertions::const_assert_eq!(
            std::mem::size_of::<Option<Atom>>(),
            std::mem::size_of::<Atom>()
        ); // Ensure no padding, and ensure that transmute is a safe operation.
        static_assertions::const_assert_eq!(std::mem::size_of::<Atom>(), 2);
        // SAFETY: `len()` returns the number of non-`None` atoms in `self.atoms`.
        //         We also know that `Option<Atom>` has the same size as `Atom`, so we can safely transmute.
        unsafe { std::slice::from_raw_parts(self.atoms.as_ptr().cast(), self.len() as usize) }
    }

    /// Fast check if `self` is a prefix of `other`.
    ///
    /// # Examples
    /// ```
    /// use jaune::{Scope, Atom};
    /// assert!(Scope::new("a.b").unwrap().is_prefix_of(&Scope::new("a.b.c").unwrap()));
    /// assert!(Scope::new("a.b").unwrap().is_prefix_of(&Scope::new("a.b").unwrap()));
    /// assert!(!Scope::new("a.b.c").unwrap().is_prefix_of(&Scope::new("a.b").unwrap()));
    /// ```
    pub fn is_prefix_of(self, other: &Scope) -> bool {
        for i in 0..MAX_ATOMS {
            let self_atom = self.atoms[i];
            let other_atom = other.atoms[i];
            match (self_atom, other_atom) {
                (Some(self_atom), Some(other_atom)) => {
                    if self_atom != other_atom {
                        return false;
                    }
                }
                (Some(_), None) => return false,
                (None, None | Some(_)) => break,
            }
        }
        true
    }

    /// Returns the number of [`Atom`]s in the scope.
    pub fn len(self) -> u8 {
        // SAFETY: `as_slice` relies on this method to determine the length of the slice. Ensure consistency.
        self.atoms
            .iter()
            .position(|a| a.is_none())
            .unwrap_or(self.atoms.len()) as u8
    }
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Scope(")?;
        let atoms = self.as_slice();
        for (i, atom) in atoms.iter().enumerate() {
            write!(f, "{}", Into::<NonZeroU16>::into(*atom))?;
            if i != atoms.len() - 1 {
                write!(f, ".")?;
            }
        }
        write!(f, ":")?;
        for (i, atom) in atoms.iter().enumerate() {
            match atom.try_to_string() {
                Some(s) => write!(f, "{:?}", s)?,
                None => write!(f, "<unknown>")?,
            }
            if i != atoms.len() - 1 {
                write!(f, ".")?;
            }
        }
        write!(f, ")")
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let atoms = self.as_slice();
        for (i, atom) in atoms.iter().enumerate() {
            write!(f, "{}", atom)?;
            if i != atoms.len() - 1 {
                write!(f, ".")?;
            }
        }
        Ok(())
    }
}

impl FromStr for Scope {
    type Err = ScopeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Scope::new(s)
    }
}

impl serde::Serialize for Scope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for Scope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Scope::new(&s).map_err(|e| serde::de::Error::custom(format!("Invalid scope: {:?}", e)))
    }
}

/// A stack of [`Scope`]s for representing a hierarchy in a token of text. The last scope is the most specific.
///
/// Parsed and displayed as a space-separated list of scopes, e.g. `source.rust meta.function.call.rust entity.name.function.rust` for a function name in Rust.
/// Or, for a more complex example: `text.html.markdown markup.fenced_code.block.markdown meta.embedded.block.html source.css meta.property-list.css meta.property-value.css constant.other.color.rgb-value.hex.css`
/// for a hex color in a CSS block inside a fenced HTML code block in Markdown.
///
/// This derefs to a `Vec<Scope>`, so all slice and vector methods are available.
///
/// # Examples
///
/// ```rust
/// # use jaune::{Scope, SimpleScopeStack};
/// # use std::str::FromStr;
/// let scope_stack = SimpleScopeStack::from_str("source.rust meta.function.call.rust entity.name.function.rust").unwrap();
/// assert_eq!(scope_stack.to_string(), "source.rust meta.function.call.rust entity.name.function.rust");
/// assert_eq!(scope_stack.len(), 3);
/// assert_eq!(scope_stack[0], Scope::new("source.rust").unwrap());
/// assert_eq!(scope_stack[1], Scope::new("meta.function.call.rust").unwrap());
/// assert_eq!(scope_stack[2], Scope::new("entity.name.function.rust").unwrap());
/// ```
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct SimpleScopeStack(Vec<Scope>);

impl SimpleScopeStack {
    /// Creates a new empty simple scope stack.
    pub fn new() -> Self {
        Self::default()
    }
}

impl From<Vec<Scope>> for SimpleScopeStack {
    fn from(v: Vec<Scope>) -> Self {
        Self(v)
    }
}

impl Into<Vec<Scope>> for SimpleScopeStack {
    fn into(self) -> Vec<Scope> {
        self.0
    }
}

impl Deref for SimpleScopeStack {
    type Target = Vec<Scope>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SimpleScopeStack {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Display for SimpleScopeStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, scope) in self.0.iter().enumerate() {
            write!(f, "{}", scope)?;
            if i != self.0.len() - 1 {
                write!(f, " ")?;
            }
        }
        Ok(())
    }
}

impl Debug for SimpleScopeStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SimpleScopeStack(")?;
        for (i, scope) in self.0.iter().enumerate() {
            write!(f, "{:?}", scope)?;
            if i != self.0.len() - 1 {
                write!(f, ", ")?;
            }
        }
        write!(f, ")")
    }
}

impl FromStr for SimpleScopeStack {
    type Err = ScopeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let scopes: Result<Vec<Scope>, ScopeParseError> = s
            .split_whitespace()
            .map(|scope_str| Scope::new(scope_str))
            .collect();
        Ok(SimpleScopeStack(scopes?))
    }
}

/// A stack of [`Scope`]s, along with a stack of cleared scope stacks.
///
/// This is used to represent the current scope stack during tokenization. This is slightly more complex
/// than [`SimpleScopeStack`] because of the need to handle clearing and restoring scope stacks. See [`SimpleScopeStack`]
/// for more information.
///
/// # Examples
/// ```
/// # use jaune::{Scope, SimpleScopeStack, ScopeStack};
/// let mut stack = ScopeStack::new();
/// stack.push(Scope::new("source.rust").unwrap());
/// stack.push(Scope::new("meta.function").unwrap());
/// assert_eq!(stack.scope_stack.len(), 2);
///
/// stack.clear_push();
/// assert_eq!(stack.scope_stack.len(), 0);
/// assert_eq!(stack.clear_stacks.len(), 1);
///
/// stack.push(Scope::new("meta.string").unwrap());
/// assert_eq!(stack.scope_stack.len(), 1);
///
/// stack.clear_pop();
/// assert_eq!(stack.scope_stack.len(), 2);
/// assert_eq!(stack.clear_stacks.len(), 0);
/// ```
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct ScopeStack {
    /// The current stack of [`Scope`]s used for highlighting.
    ///
    /// e.g. `source.rust meta.function.call.rust entity.name.function.rust` for a function name in Rust.
    ///
    /// A more complex example: `text.html.markdown markup.fenced_code.block.markdown meta.embedded.block.html source.css meta.property-list.css meta.property-value.css constant.other.color.rgb-value.hex.css`
    /// for a hex color in a CSS block inside a fenced HTML code block in Markdown.
    pub scope_stack: SimpleScopeStack,
    /// The stack of [`SimpleScopeStack`]s that have been cleared that can be restored later.
    ///
    /// This can happen when a grammar rule specifies to clear the scope stack.
    pub clear_stacks: Vec<SimpleScopeStack>,
}

impl ScopeStack {
    /// Creates a new empty [`ScopeStack`].
    pub fn new() -> Self {
        Default::default()
    }

    /// Pushes a new scope onto the stack.
    pub fn push(&mut self, scope: Scope) {
        self.scope_stack.push(scope);
    }

    /// Pops the top scope from the stack.
    ///
    /// Returns `None` if the stack is empty.
    pub fn pop(&mut self) -> Option<Scope> {
        self.scope_stack.pop()
    }

    /// Clear the scope stack entirely.
    pub fn clear(&mut self) {
        self.scope_stack.clear();
        self.clear_stacks.clear();
    }

    /// Clears the current scope stack, pushing it onto the `clear_stacks`.
    pub fn clear_push(&mut self) {
        let current_scopes = std::mem::take(&mut self.scope_stack);
        self.clear_stacks.push(current_scopes);
    }

    /// Restores the most recently cleared scope stack.
    ///
    /// Returns `false` and clears the current scope if there is no cleared
    /// scope stack to restore.
    pub fn clear_pop(&mut self) -> bool {
        if let Some(restored_scopes) = self.clear_stacks.pop() {
            self.scope_stack = restored_scopes;
            true
        } else {
            self.scope_stack.clear();
            false
        }
    }
}

impl From<SimpleScopeStack> for ScopeStack {
    fn from(scope_stack: SimpleScopeStack) -> Self {
        Self {
            scope_stack,
            clear_stacks: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_stack_display() {
        let scopes = SimpleScopeStack::from(vec![
            Scope::new("meta.function.rust").unwrap(),
            Scope::new("meta.block").unwrap(),
        ]);
        assert_eq!(scopes.to_string(), "meta.function.rust meta.block");
    }
}

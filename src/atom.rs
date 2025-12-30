use std::{
    num::NonZeroU16,
    sync::{LazyLock, RwLock},
};

pub use bimap::BiMap;

const ONE_U16_NONZERO: NonZeroU16 = NonZeroU16::new(1).unwrap();

/// An interned string repository for [`Atom`]s.
///
/// Use [`ATOM_REPO`] to access this.
pub struct AtomRepository {
    map: BiMap<String, Atom>, // TODO: Vec<String> + HashMap for better performance?
    next_id: NonZeroU16,
}

impl AtomRepository {
    /// Creates a new, empty [`AtomRepository`].
    ///
    /// This is not intended to be used directly, as [`Atom`]s generated from it
    /// will not correctly match to [`Atom`]s in other [`AtomRepository`]s, including
    /// the global [`ATOM_REPO`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of [`Atom`]s in the repository.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    fn check_string(s: &str) -> Result<&str, AtomParseError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(AtomParseError::EmptyString);
        }
        if s.contains('.') {
            return Err(AtomParseError::InvalidDotCharacter);
        }
        Ok(s)
    }

    /// Try to get an [`Atom`] from the repository by its string, or return `None` if it doesn't exist.
    pub fn try_get(&self, s: &str) -> Option<Atom> {
        let s = Self::check_string(s).ok()?;
        self.map.get_by_left(s).copied()
    }

    /// Try to add a string to the repository, or return the existing [`Atom`].
    pub fn try_get_or_add(&mut self, s: &str) -> Result<Atom, AtomParseError> {
        let s = Self::check_string(s)?;
        if let Some(&atom) = self.map.get_by_left(s) {
            return Ok(atom);
        } else {
            let atom = Atom(self.next_id);
            self.next_id = match self.next_id.checked_add(1) {
                Some(id) => id,
                None => return Err(AtomParseError::MaxAtomsReached),
            };
            println!("Inserting {:?} with ID {:?}", &s, atom.0);
            self.map.insert(s.to_string(), atom);
            return Ok(atom);
        }
    }
}

impl Default for AtomRepository {
    fn default() -> Self {
        Self {
            map: BiMap::new(),
            next_id: ONE_U16_NONZERO,
        }
    }
}

/// The global interned string repository for [`Atom`]s.
pub static ATOM_REPO: LazyLock<RwLock<AtomRepository>> =
    LazyLock::new(|| RwLock::new(AtomRepository::new()));

/// Error type for [`Atom`] parsing.
#[derive(Debug, thiserror::Error)]
pub enum AtomParseError {
    /// Maximum number of unique [`Atom`]s has been reached.
    ///
    /// Currently this is `u16::MAX - 2`.
    ///
    /// See [`AtomRepository`] for more information.
    #[error("maximum number of unique atoms reached")]
    MaxAtomsReached,
    /// [`Atom`] contains a `.` character, which is not allowed.
    #[error("atom cannot contain '.' character")]
    InvalidDotCharacter,
    /// [`Atom`] is an empty string.
    #[error("atom cannot be an empty string")]
    EmptyString,
}

/// A 16-bit integer representing an interned string (e.g., `function` -> `42`).
///
/// Non-zero to allow for `Option<Atom>` to be 2 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Atom(NonZeroU16);

impl Atom {
    /// Creates a new [`Atom`] from a string. If the string has already been interned,
    /// returns the existing [`Atom`].
    ///
    /// Conventionally [`Atom`]s are lowercase. This is not enforced, but may lead to
    /// unexpected behavior if not followed. Whitespace is stripped from the start and end of the string.
    ///
    /// # Errors
    /// - [`AtomParseError::MaxAtomsReached`] if the maximum number of unique atoms has been reached.
    /// - [`AtomParseError::InvalidDotCharacter`] if the string contains a `.` character.
    /// - [`AtomParseError::EmptyString`] if the string is empty.
    ///
    /// # Examples
    /// ```
    /// # use jaune::Atom;
    /// let atom1 = Atom::new("function").unwrap();
    /// let atom2 = Atom::new("function").unwrap();
    /// let atom3 = Atom::new(" function ").unwrap();
    /// assert_eq!(atom1, atom2);
    /// assert_eq!(atom1, atom3);
    /// ```
    pub fn new(s: &str) -> Result<Self, AtomParseError> {
        {
            // First try to read from the interner without locking it for writing.
            let repo = ATOM_REPO.read().expect("interner lock poisoned");
            if let Some(atom) = repo.try_get(s) {
                return Ok(atom);
            }
        }
        let mut repo = ATOM_REPO.write().expect("interner lock poisoned");
        repo.try_get_or_add(s)
    }

    /// Tries to return the string corresponding to this [`Atom`].
    ///
    /// Returns `None` if the [`Atom`] is not found in the interner.
    pub fn try_to_string(&self) -> Option<String> {
        let repo = ATOM_REPO.read().expect("interner lock poisoned");
        repo.map.get_by_right(&self).cloned()
    }

    /// Returns the string corresponding to this [`Atom`].
    ///
    /// # Panics
    /// Panics if the [`Atom`] is not found in the interner.
    pub fn to_string(&self) -> String {
        self.try_to_string().expect("Atom not found in interner")
    }
}

impl std::fmt::Debug for Atom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self.try_to_string() {
            Some(s) => format!("{:?}", s),
            None => "<unknown>".to_string(),
        };
        write!(f, "Atom({}:{})", self.0, s)
    }
}

impl std::fmt::Display for Atom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.try_to_string() {
            Some(s) => write!(f, "{}", s),
            None => write!(f, "<unknown>"),
        }
    }
}

impl Into<NonZeroU16> for Atom {
    fn into(self) -> NonZeroU16 {
        self.0
    }
}

impl From<NonZeroU16> for Atom {
    fn from(id: NonZeroU16) -> Self {
        Self(id)
    }
}

impl std::str::FromStr for Atom {
    type Err = AtomParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Atom::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atom_eq_basic() {
        let atom1 = Atom(NonZeroU16::new(1).unwrap());
        assert_eq!(atom1, atom1);
        let atom2 = Atom(NonZeroU16::new(1).unwrap());
        assert_eq!(atom1, atom2);
        let atom3 = Atom(NonZeroU16::new(2).unwrap());
        assert_ne!(atom1, atom3);
        assert_ne!(atom2, atom3);
    }
}

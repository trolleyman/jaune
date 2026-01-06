use std::{num::NonZeroU16, str::FromStr};

use crate::{Atom, atom::AtomParseError};

/// Error type for [`Scope`] creation.
#[derive(Debug, thiserror::Error)]
pub enum ScopeParseError {
    /// More than 8 [`Atom`]s were provided.
    #[error("scope can only hold up to 8 atoms")]
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
/// Stores up to 8 [`Atom`]s. Uses 16 bytes (2 bytes per [`Atom`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scope {
    /// The atoms in the scope. `None` indicates the end of the scope.
    atoms: [Option<Atom>; 8],
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
    /// - [`ScopeParseError::TooManyAtoms`] if more than 8 [`Atom`]s are provided.
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
                if atoms.len() > 8 {
                    return Err(ScopeParseError::TooManyAtoms);
                }
                Scope::from_slice(&atoms)
            }
        }
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
    /// Returns `ScopeParseError::TooManyAtoms` if more than 8 atoms are provided.
    pub fn from_slice(atoms: &[Atom]) -> Result<Self, ScopeParseError> {
        if atoms.len() > 8 {
            return Err(ScopeParseError::TooManyAtoms);
        }
        let mut arr = [None; 8];
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
        for i in 0..8 {
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

/// A stack of [`Scope`]s, along with a set of clear stacks.
///
/// This is used to represent the current scope stack during tokenization.
#[derive(Clone, PartialEq, Debug)]
pub struct ScopeStack {
    pub scopes: Vec<Scope>,
}

impl ScopeStack
{

}

// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: Apache-2.0

//! Reusable validated string, path, and collection configuration values.

use std::{
    collections::HashSet,
    fmt,
    hash::Hash,
    ops::Deref,
    path::{Path, PathBuf},
};

use garde::{Error, Report, Validate};
use serde::{Deserialize, Serialize};

/// A string that must not be exactly empty.
///
/// Whitespace is preserved and considered a value. Use [`NonBlankString`] when
/// whitespace-only strings must be rejected.
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    /// Returns the string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the owned string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Deref for NonEmptyString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for NonEmptyString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for NonEmptyString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for NonEmptyString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for NonEmptyString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<Box<str>> for NonEmptyString {
    fn from(value: Box<str>) -> Self {
        Self(value.into())
    }
}

impl From<NonEmptyString> for String {
    fn from(value: NonEmptyString) -> Self {
        value.into_string()
    }
}

impl Validate for NonEmptyString {
    type Context = ();

    fn validate_into(
        &self,
        (): &Self::Context,
        parent: &mut dyn FnMut() -> garde::Path,
        report: &mut Report,
    ) {
        if self.0.is_empty() {
            report.append(parent(), Error::new("must not be empty"));
        }
    }
}

/// A string that contains at least one non-whitespace character.
///
/// The input is never trimmed or otherwise changed.
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct NonBlankString(String);

impl NonBlankString {
    /// Returns the string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the owned string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Deref for NonBlankString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for NonBlankString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for NonBlankString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for NonBlankString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for NonBlankString {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<Box<str>> for NonBlankString {
    fn from(value: Box<str>) -> Self {
        Self(value.into())
    }
}

impl From<NonBlankString> for String {
    fn from(value: NonBlankString) -> Self {
        value.into_string()
    }
}

impl Validate for NonBlankString {
    type Context = ();

    fn validate_into(
        &self,
        (): &Self::Context,
        parent: &mut dyn FnMut() -> garde::Path,
        report: &mut Report,
    ) {
        if self.0.trim().is_empty() {
            report.append(parent(), Error::new("must not be blank"));
        }
    }
}

/// A path that must not be empty.
///
/// This type intentionally imposes no absolute-path, existence, or
/// normalization policy.
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct NonEmptyPath(PathBuf);

impl NonEmptyPath {
    /// Returns the path slice.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Returns the owned path buffer.
    #[must_use]
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl Deref for NonEmptyPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.as_path()
    }
}

impl AsRef<Path> for NonEmptyPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl From<PathBuf> for NonEmptyPath {
    fn from(value: PathBuf) -> Self {
        Self(value)
    }
}

impl From<&Path> for NonEmptyPath {
    fn from(value: &Path) -> Self {
        Self(value.to_owned())
    }
}

impl From<&str> for NonEmptyPath {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl From<String> for NonEmptyPath {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl From<NonEmptyPath> for PathBuf {
    fn from(value: NonEmptyPath) -> Self {
        value.into_path_buf()
    }
}

impl Validate for NonEmptyPath {
    type Context = ();

    fn validate_into(
        &self,
        (): &Self::Context,
        parent: &mut dyn FnMut() -> garde::Path,
        report: &mut Report,
    ) {
        if self.0.as_os_str().is_empty() {
            report.append(parent(), Error::new("must not be empty"));
        }
    }
}

/// An order-preserving configuration collection with a minimum item count.
///
/// Deserialization and serialization use an ordinary sequence. Duplicate
/// values are retained until validation so configuration errors can be
/// reported instead of silently changing the supplied value.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Set<T, const MIN_ITEMS: usize = 0>(Vec<T>);

/// An order-preserving collection that must contain at least one item.
pub type NonEmptySet<T> = Set<T, 1>;

impl<T, const MIN_ITEMS: usize> Set<T, MIN_ITEMS> {
    /// Returns the items in declaration order.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    /// Returns an iterator in declaration order.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }

    /// Returns the owned items in declaration order.
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

impl<T, const MIN_ITEMS: usize> Deref for Set<T, MIN_ITEMS> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T, const MIN_ITEMS: usize> AsRef<[T]> for Set<T, MIN_ITEMS> {
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, const MIN_ITEMS: usize> From<Vec<T>> for Set<T, MIN_ITEMS> {
    fn from(value: Vec<T>) -> Self {
        Self(value)
    }
}

impl<T, const MIN_ITEMS: usize, const N: usize> From<[T; N]> for Set<T, MIN_ITEMS> {
    fn from(value: [T; N]) -> Self {
        Self(Vec::from(value))
    }
}

impl<T, const MIN_ITEMS: usize> From<Set<T, MIN_ITEMS>> for Vec<T> {
    fn from(value: Set<T, MIN_ITEMS>) -> Self {
        value.into_vec()
    }
}

impl<T, const MIN_ITEMS: usize> IntoIterator for Set<T, MIN_ITEMS> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T, const MIN_ITEMS: usize> IntoIterator for &'a Set<T, MIN_ITEMS> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T, const MIN_ITEMS: usize> Validate for Set<T, MIN_ITEMS>
where
    T: Eq + Hash,
{
    type Context = ();

    fn validate_into(
        &self,
        (): &Self::Context,
        parent: &mut dyn FnMut() -> garde::Path,
        report: &mut Report,
    ) {
        if self.0.len() < MIN_ITEMS {
            report.append(
                parent(),
                Error::new(format!("must contain at least {MIN_ITEMS} items")),
            );
        }
        if self.0.iter().collect::<HashSet<_>>().len() != self.0.len() {
            report.append(parent(), Error::new("must not contain duplicate values"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, path::Path};

    use garde::Validate as _;
    use serde::{Deserialize, Serialize};

    use super::{NonBlankString, NonEmptyPath, NonEmptySet, NonEmptyString, Set};

    #[derive(Deserialize, Serialize)]
    struct TransparentValues {
        nonempty: NonEmptyString,
        nonblank: NonBlankString,
        path: NonEmptyPath,
    }

    #[derive(Deserialize, garde::Validate)]
    struct CheckedValues {
        #[garde(dive)]
        nonempty: NonEmptyString,
        #[garde(dive)]
        nonblank: NonBlankString,
        #[garde(dive)]
        path: NonEmptyPath,
    }

    #[derive(Deserialize, Serialize, garde::Validate)]
    struct CheckedSet {
        #[garde(dive)]
        values: NonEmptySet<usize>,
    }

    #[test]
    fn transparently_decodes_strings_and_paths_in_all_supported_formats() {
        let json: TransparentValues = serde_json::from_str(
            r#"{"nonempty":" value ","nonblank":" text ","path":"relative/path"}"#,
        )
        .unwrap();
        let toml: TransparentValues =
            toml::from_str("nonempty = ' value '\nnonblank = ' text '\npath = 'relative/path'\n")
                .unwrap();
        let yaml: TransparentValues = serde_yaml_ng::from_str(
            "nonempty: ' value '\nnonblank: ' text '\npath: relative/path\n",
        )
        .unwrap();

        assert_eq!(json.nonempty.as_str(), " value ");
        assert_eq!(toml.nonblank.as_str(), " text ");
        assert_eq!(yaml.path.as_path(), Path::new("relative/path"));
        assert_eq!(
            serde_json::to_string(&json).unwrap(),
            r#"{"nonempty":" value ","nonblank":" text ","path":"relative/path"}"#
        );
    }

    #[test]
    fn validates_empty_blank_and_empty_path_values() {
        let values: CheckedValues =
            serde_json::from_str(r#"{"nonempty":"","nonblank":" \t ","path":""}"#).unwrap();
        let report = values.validate().unwrap_err().to_string();

        assert!(report.contains("nonempty: must not be empty"));
        assert!(report.contains("nonblank: must not be blank"));
        assert!(report.contains("path: must not be empty"));
        assert!(NonEmptyString::from(" ").validate().is_ok());
        assert!(NonBlankString::from(" ").validate().is_err());
        assert!(NonEmptyPath::from("relative/path").validate().is_ok());
    }

    #[test]
    fn preserves_set_declaration_order_in_all_supported_formats() {
        let json: Set<usize> = serde_json::from_str("[3, 1, 2]").unwrap();
        let toml: Set<usize> = toml::from_str("values = [3, 1, 2]")
            .map(|wrapper: TomlSet| wrapper.values)
            .unwrap();
        let yaml: Set<usize> = serde_yaml_ng::from_str("- 3\n- 1\n- 2\n").unwrap();

        assert_eq!(json.as_slice(), [3, 1, 2]);
        assert_eq!(toml.as_slice(), [3, 1, 2]);
        assert_eq!(yaml.as_slice(), [3, 1, 2]);
        assert_eq!(serde_json::to_string(&json).unwrap(), "[3,1,2]");
    }

    #[derive(Deserialize)]
    struct TomlSet {
        values: Set<usize>,
    }

    #[test]
    fn reports_set_minimum_and_duplicates_at_the_containing_field() {
        let empty: CheckedSet = serde_json::from_str(r#"{"values":[]}"#).unwrap();
        let duplicate: CheckedSet = serde_json::from_str(r#"{"values":[1,1]}"#).unwrap();

        let empty_report = empty.validate().unwrap_err().to_string();
        let duplicate_report = duplicate.validate().unwrap_err().to_string();
        assert!(empty_report.contains("values: must contain at least 1 items"));
        assert!(duplicate_report.contains("values: must not contain duplicate values"));
    }

    #[test]
    fn non_empty_set_of_nonzero_usizes_rejects_zero_during_deserialization() {
        assert!(serde_json::from_str::<NonEmptySet<NonZeroUsize>>("[0]").is_err());
        let values: NonEmptySet<NonZeroUsize> = serde_json::from_str("[1,2]").unwrap();
        assert_eq!(
            values.iter().map(|value| value.get()).collect::<Vec<_>>(),
            [1, 2]
        );
    }
}

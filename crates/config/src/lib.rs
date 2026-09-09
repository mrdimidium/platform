// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: Apache-2.0

//! Load and validate one explicitly specified service configuration file.

use std::{
    error::Error as StdError,
    fmt, io,
    path::{Path, PathBuf},
    string::FromUtf8Error,
};

use garde::Validate;
use serde::de::DeserializeOwned;

mod quantities;
mod values;

pub use quantities::{
    ByteSize, Duration, NonZeroByteSize, NonZeroDuration, U16ByteSize, U32ByteSize, UsizeByteSize,
};
pub use values::{NonBlankString, NonEmptyPath, NonEmptySet, NonEmptyString, Set};

/// A configuration file format selected from a file extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Json,
    Toml,
    Yaml,
}

impl fmt::Display for Format {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
        })
    }
}

/// Identifies the service and file from which configuration was loaded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    name: String,
    path: PathBuf,
    format: Format,
}

impl Metadata {
    /// Returns the service name supplied to [`load`].
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the exact path supplied to [`load`].
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the normalized format selected from the path extension.
    #[must_use]
    pub const fn format(&self) -> Format {
        self.format
    }
}

/// A validated configuration value and the metadata for its source file.
#[derive(Debug)]
pub struct Loaded<T> {
    config: T,
    metadata: Metadata,
}

impl<T> Loaded<T> {
    /// Returns the validated configuration value.
    #[must_use]
    pub fn config(&self) -> &T {
        &self.config
    }

    /// Returns metadata for the loaded file.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Splits this result into its configuration value and metadata.
    #[must_use]
    pub fn into_parts(self) -> (T, Metadata) {
        (self.config, self.metadata)
    }
}

/// An error encountered while loading or validating a configuration file.
#[derive(Debug)]
pub enum Error {
    /// The supplied path has no supported extension.
    UnsupportedFormat { name: String, path: PathBuf },
    /// Reading the supplied file failed.
    Read {
        source: io::Error,
        metadata: Metadata,
    },
    /// The supplied file was not valid UTF-8.
    Utf8 {
        source: FromUtf8Error,
        metadata: Metadata,
    },
    /// JSON decoding failed.
    Json {
        source: Box<serde_json::Error>,
        metadata: Metadata,
    },
    /// TOML decoding failed.
    Toml {
        source: Box<toml::de::Error>,
        metadata: Metadata,
    },
    /// YAML decoding failed.
    Yaml {
        source: Box<serde_yaml_ng::Error>,
        metadata: Metadata,
    },
    /// Garde validation failed.
    Validation {
        source: Box<garde::Report>,
        metadata: Metadata,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { name, path } => write!(
                formatter,
                "configuration for service {name:?} at {} must have a .json, .toml, .yaml, or .yml extension",
                path.display()
            ),
            Self::Read { source, metadata } => write!(
                formatter,
                "could not read configuration for service {:?} at {}: {source}",
                metadata.name(),
                metadata.path().display()
            ),
            Self::Utf8 { source, metadata } => write!(
                formatter,
                "configuration for service {:?} at {} is not valid UTF-8: {source}",
                metadata.name(),
                metadata.path().display()
            ),
            Self::Json { source, metadata } => write!(
                formatter,
                "could not decode JSON configuration for service {:?} at {}: {source}",
                metadata.name(),
                metadata.path().display()
            ),
            Self::Toml { source, metadata } => write!(
                formatter,
                "could not decode TOML configuration for service {:?} at {}: {source}",
                metadata.name(),
                metadata.path().display()
            ),
            Self::Yaml { source, metadata } => write!(
                formatter,
                "could not decode YAML configuration for service {:?} at {}: {source}",
                metadata.name(),
                metadata.path().display()
            ),
            Self::Validation { source, metadata } => write!(
                formatter,
                "{} configuration for service {:?} at {} failed validation: {source}",
                metadata.format(),
                metadata.name(),
                metadata.path().display()
            ),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::UnsupportedFormat { .. } => None,
            Self::Read { source, .. } => Some(source),
            Self::Utf8 { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Toml { source, .. } => Some(source),
            Self::Yaml { source, .. } => Some(source),
            Self::Validation { source, .. } => Some(source),
        }
    }
}

/// Loads, deserializes, and validates one explicitly supplied configuration file.
///
/// The format is selected only from a case-insensitive `.json`, `.toml`, `.yaml`,
/// or `.yml` extension. The path is neither canonicalized nor otherwise changed.
/// Validation uses [`Validate::validate`] and therefore requires the configuration
/// type's validation context to implement [`Default`].
///
/// # Errors
///
/// Returns [`Error`] when the format is unsupported, the file cannot be read or
/// decoded, or the deserialized value fails validation.
pub async fn load<T>(name: impl Into<String>, path: impl AsRef<Path>) -> Result<Loaded<T>, Error>
where
    T: DeserializeOwned + Validate,
    T::Context: Default,
{
    let name = name.into();
    let path = path.as_ref().to_owned();
    let Some(format) = format_for(&path) else {
        return Err(Error::UnsupportedFormat { name, path });
    };
    let metadata = Metadata { name, path, format };
    let bytes = tokio::fs::read(metadata.path())
        .await
        .map_err(|source| Error::Read {
            source,
            metadata: metadata.clone(),
        })?;
    let contents = String::from_utf8(bytes).map_err(|source| Error::Utf8 {
        source,
        metadata: metadata.clone(),
    })?;
    let config: T = match format {
        Format::Json => serde_json::from_str(&contents).map_err(|source| Error::Json {
            source: Box::new(source),
            metadata: metadata.clone(),
        })?,
        Format::Toml => toml::from_str(&contents).map_err(|source| Error::Toml {
            source: Box::new(source),
            metadata: metadata.clone(),
        })?,
        Format::Yaml => serde_yaml_ng::from_str(&contents).map_err(|source| Error::Yaml {
            source: Box::new(source),
            metadata: metadata.clone(),
        })?,
    };
    config.validate().map_err(|source| Error::Validation {
        source: Box::new(source),
        metadata: metadata.clone(),
    })?;

    Ok(Loaded { config, metadata })
}

fn format_for(path: &Path) -> Option<Format> {
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("json") {
        Some(Format::Json)
    } else if extension.eq_ignore_ascii_case("toml") {
        Some(Format::Toml)
    } else if extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml") {
        Some(Format::Yaml)
    } else {
        None
    }
}

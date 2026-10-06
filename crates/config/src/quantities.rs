// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

//! Human-readable, validated quantitative configuration values.

use std::{any::type_name, fmt, marker::PhantomData, time::Duration as StdDuration};

use garde::{Error, Report, Validate};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// A human-readable duration with a minimum value expressed in nanoseconds.
///
/// Deserialization accepts only strings understood by [`humantime::parse_duration`],
/// such as `"6h"` and `"10s"`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Duration<const MIN_NANOS: u128 = 0>(StdDuration);

/// A duration that must be greater than zero.
pub type NonZeroDuration = Duration<1>;

impl<const MIN_NANOS: u128> Duration<MIN_NANOS> {
    /// A zero duration. It fails validation when `MIN_NANOS` is nonzero.
    pub const ZERO: Self = Self(StdDuration::ZERO);

    /// Creates a duration from whole seconds.
    #[must_use]
    pub const fn from_secs(secs: u64) -> Self {
        Self(StdDuration::from_secs(secs))
    }

    /// Creates a duration from whole milliseconds.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(StdDuration::from_millis(millis))
    }

    /// Returns the underlying standard-library duration.
    #[must_use]
    pub const fn get(self) -> StdDuration {
        self.0
    }

    /// Returns the underlying standard-library duration.
    #[must_use]
    pub const fn as_std(&self) -> StdDuration {
        self.0
    }
}

impl<const MIN_NANOS: u128> Serialize for Duration<MIN_NANOS> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&humantime::format_duration(self.0).to_string())
    }
}

impl<'de, const MIN_NANOS: u128> Deserialize<'de> for Duration<MIN_NANOS> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        humantime::parse_duration(&value)
            .map(Self)
            .map_err(de::Error::custom)
    }
}

impl<const MIN_NANOS: u128> Validate for Duration<MIN_NANOS> {
    type Context = ();

    fn validate_into(
        &self,
        (): &Self::Context,
        parent: &mut dyn FnMut() -> garde::Path,
        report: &mut Report,
    ) {
        if self.0.as_nanos() < MIN_NANOS {
            report.append(
                parent(),
                Error::new(format!("must be at least {MIN_NANOS} nanoseconds")),
            );
        }
    }
}

/// A human-readable byte size with a minimum value and an intended target type.
///
/// Deserialization accepts only strings with a byte unit understood by
/// [`bytesize::ByteSize`], such as `"10Gb"`, `"64 MiB"`, and `"1KiB"`.
/// `T` is checked during garde validation, so target-width overflow is reported
/// as a configuration validation error rather than a decoding error.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ByteSize<T = u64, const MIN_BYTES: u64 = 0> {
    bytes: u64,
    target: PhantomData<fn() -> T>,
}

/// A byte size that must be greater than zero.
pub type NonZeroByteSize<T = u64> = ByteSize<T, 1>;
/// A byte size validated for conversion to `usize`.
pub type UsizeByteSize<const MIN_BYTES: u64 = 0> = ByteSize<usize, MIN_BYTES>;
/// A byte size validated for conversion to `u32`.
pub type U32ByteSize<const MIN_BYTES: u64 = 0> = ByteSize<u32, MIN_BYTES>;
/// A byte size validated for conversion to `u16`.
pub type U16ByteSize<const MIN_BYTES: u64 = 0> = ByteSize<u16, MIN_BYTES>;

impl<T, const MIN_BYTES: u64> ByteSize<T, MIN_BYTES> {
    /// A zero byte size. It fails validation when `MIN_BYTES` is nonzero.
    pub const ZERO: Self = Self::b(0);

    /// Creates a byte size from bytes.
    #[must_use]
    pub const fn b(bytes: u64) -> Self {
        Self {
            bytes,
            target: PhantomData,
        }
    }

    /// Creates a byte size from decimal kilobytes.
    #[must_use]
    pub const fn kb(value: u64) -> Self {
        Self::b(value * bytesize::KB)
    }

    /// Creates a byte size from binary kibibytes.
    #[must_use]
    pub const fn kib(value: u64) -> Self {
        Self::b(value * bytesize::KIB)
    }

    /// Creates a byte size from decimal megabytes.
    #[must_use]
    pub const fn mb(value: u64) -> Self {
        Self::b(value * bytesize::MB)
    }

    /// Creates a byte size from binary mebibytes.
    #[must_use]
    pub const fn mib(value: u64) -> Self {
        Self::b(value * bytesize::MIB)
    }

    /// Creates a byte size from decimal gigabytes.
    #[must_use]
    pub const fn gb(value: u64) -> Self {
        Self::b(value * bytesize::GB)
    }

    /// Creates a byte size from binary gibibytes.
    #[must_use]
    pub const fn gib(value: u64) -> Self {
        Self::b(value * bytesize::GIB)
    }

    /// Returns the number of bytes.
    #[must_use]
    pub const fn as_u64(&self) -> u64 {
        self.bytes
    }

    /// Converts to the configured target type.
    ///
    /// This is infallible after successful [`Validate::validate`] validation.
    ///
    /// # Panics
    ///
    /// Panics when called before validation and the value does not fit `T`.
    #[must_use]
    pub fn get(self) -> T
    where
        T: TryFrom<u64>,
        T::Error: fmt::Debug,
    {
        T::try_from(self.bytes).expect("ByteSize target conversion was validated")
    }
}

impl<T, const MIN_BYTES: u64> fmt::Debug for ByteSize<T, MIN_BYTES> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ByteSize")
            .field("bytes", &self.bytes)
            .field("target", &type_name::<T>())
            .field("minimum_bytes", &MIN_BYTES)
            .finish()
    }
}

impl<T, const MIN_BYTES: u64> Serialize for ByteSize<T, MIN_BYTES> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&bytesize::ByteSize::b(self.bytes).to_string())
    }
}

impl<'de, T, const MIN_BYTES: u64> Deserialize<'de> for ByteSize<T, MIN_BYTES> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if !value
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        {
            return Err(de::Error::custom("byte size must include a unit suffix"));
        }
        value
            .parse::<bytesize::ByteSize>()
            .map(|size| Self::b(size.as_u64()))
            .map_err(de::Error::custom)
    }
}

impl<T, const MIN_BYTES: u64> Validate for ByteSize<T, MIN_BYTES>
where
    T: TryFrom<u64>,
{
    type Context = ();

    fn validate_into(
        &self,
        (): &Self::Context,
        parent: &mut dyn FnMut() -> garde::Path,
        report: &mut Report,
    ) {
        if self.bytes < MIN_BYTES {
            report.append(
                parent(),
                Error::new(format!("must be at least {MIN_BYTES} bytes")),
            );
        }
        if T::try_from(self.bytes).is_err() {
            report.append(
                parent(),
                Error::new(format!("must fit into {}", type_name::<T>())),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use garde::Validate as _;
    use serde::Deserialize;

    use super::{ByteSize, Duration, NonZeroByteSize, NonZeroDuration, U16ByteSize};

    #[derive(Deserialize)]
    struct Values {
        duration: NonZeroDuration,
        size: NonZeroByteSize,
    }

    #[derive(Deserialize, garde::Validate)]
    struct CheckedSize {
        #[garde(dive)]
        size: U16ByteSize,
    }

    #[test]
    fn deserializes_human_readable_values_in_all_supported_formats() {
        let json: Values = serde_json::from_str(r#"{"duration":"6h","size":"10Gb"}"#).unwrap();
        let toml: Values = toml::from_str("duration = '10s'\nsize = '64 MiB'\n").unwrap();
        let yaml: Values = serde_yaml_ng::from_str("duration: 1ms\nsize: 1KiB\n").unwrap();

        assert_eq!(json.duration.get().as_secs(), 6 * 60 * 60);
        assert_eq!(json.size.as_u64(), 10 * bytesize::GB);
        assert_eq!(toml.duration.get().as_secs(), 10);
        assert_eq!(toml.size.as_u64(), 64 * bytesize::MIB);
        assert_eq!(yaml.duration.get().as_millis(), 1);
        assert_eq!(yaml.size.as_u64(), bytesize::KIB);
    }

    #[test]
    fn rejects_numeric_and_unitless_values() {
        assert!(serde_json::from_str::<Values>(r#"{"duration":10,"size":"1KiB"}"#).is_err());
        assert!(serde_json::from_str::<Values>(r#"{"duration":"10s","size":1024}"#).is_err());
        assert!(toml::from_str::<Values>("duration = 10\nsize = '1KiB'\n").is_err());
        assert!(toml::from_str::<Values>("duration = '10s'\nsize = 1024\n").is_err());
        assert!(serde_yaml_ng::from_str::<Values>("duration: 10\nsize: 1KiB\n").is_err());
        assert!(serde_yaml_ng::from_str::<Values>("duration: 10s\nsize: 1024\n").is_err());
        assert!(serde_json::from_str::<Values>(r#"{"duration":"10s","size":"1024"}"#).is_err());
    }

    #[test]
    fn validates_duration_and_byte_size_minimums() {
        assert!(Duration::<1>::ZERO.validate().is_err());
        assert!(NonZeroByteSize::<u64>::ZERO.validate().is_err());
        assert!(ByteSize::<u64, 1>::ZERO.validate().is_err());
    }

    #[test]
    fn reports_target_overflow_at_the_containing_field() {
        let value: CheckedSize = serde_json::from_str(r#"{"size":"64KiB"}"#).unwrap();
        let report = value.validate().unwrap_err();
        assert!(report.to_string().contains("size"));
        assert!(report.to_string().contains("must fit into u16"));
    }

    #[test]
    fn returns_the_validated_target_type() {
        let size = ByteSize::<u16>::kib(1);
        size.validate().unwrap();
        assert_eq!(size.get(), 1024_u16);
    }
}

use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

/// Maximum accepted encoded cursor length.
pub const MAX_CURSOR_LENGTH: usize = 1_024;
/// Default number of resources requested per page.
pub const DEFAULT_PAGE_LIMIT: u16 = 100;
/// Maximum number of resources requested per page.
pub const MAX_PAGE_LIMIT: u16 = 1_000;

/// An encoded cursor whose contents are intentionally opaque to API consumers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueCursor(String);

impl OpaqueCursor {
    /// Validates an encoded cursor.
    pub fn new(value: impl Into<String>) -> Result<Self, OpaqueCursorError> {
        let value = value.into();
        if value.is_empty() {
            return Err(OpaqueCursorError::Empty);
        }
        if value.len() > MAX_CURSOR_LENGTH {
            return Err(OpaqueCursorError::TooLong);
        }
        if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(OpaqueCursorError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the encoded cursor.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the value and returns the encoded cursor.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for OpaqueCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for OpaqueCursor {
    type Err = OpaqueCursorError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for OpaqueCursor {
    type Error = OpaqueCursorError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for OpaqueCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Cursor validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OpaqueCursorError {
    #[error("cursor cannot be empty")]
    Empty,
    #[error("cursor cannot exceed {MAX_CURSOR_LENGTH} bytes")]
    TooLong,
    #[error("cursor must contain only visible ASCII characters")]
    InvalidCharacter,
}

/// Validated page size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PageLimit(u16);

impl PageLimit {
    /// Validates a page size.
    pub const fn new(value: u16) -> Result<Self, PageLimitError> {
        if value == 0 {
            Err(PageLimitError::Zero)
        } else if value > MAX_PAGE_LIMIT {
            Err(PageLimitError::TooLarge)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the page size.
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl Default for PageLimit {
    fn default() -> Self {
        Self(DEFAULT_PAGE_LIMIT)
    }
}

impl TryFrom<u16> for PageLimit {
    type Error = PageLimitError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for PageLimit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u16::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Page-size validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PageLimitError {
    #[error("page limit must be positive")]
    Zero,
    #[error("page limit cannot exceed {MAX_PAGE_LIMIT}")]
    TooLarge,
}

/// Query parameters shared by cursor-paginated collection endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CursorPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<OpaqueCursor>,
    #[serde(default)]
    pub limit: PageLimit,
}

/// A collection page and the cursor for the next page, when one exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<OpaqueCursor>,
}

impl<T> CursorPage<T> {
    /// Creates a page.
    pub fn new(items: Vec<T>, next_cursor: Option<OpaqueCursor>) -> Self {
        Self { items, next_cursor }
    }

    /// Returns whether another page is available.
    pub const fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_page_round_trips_without_exposing_cursor_structure() {
        let page = CursorPage::new(
            vec![11_i64, 12],
            Some(OpaqueCursor::new("eyJpZCI6MTJ9.signature").unwrap()),
        );

        let json = serde_json::to_string(&page).unwrap();
        assert_eq!(
            json,
            r#"{"items":[11,12],"next_cursor":"eyJpZCI6MTJ9.signature"}"#
        );
        assert_eq!(
            serde_json::from_str::<CursorPage<i64>>(&json).unwrap(),
            page
        );
        assert!(page.has_more());
    }

    #[test]
    fn cursors_reject_blank_control_and_oversized_values() {
        assert_eq!(OpaqueCursor::new(""), Err(OpaqueCursorError::Empty));
        assert_eq!(
            OpaqueCursor::new("contains spaces"),
            Err(OpaqueCursorError::InvalidCharacter)
        );
        assert_eq!(
            OpaqueCursor::new("x".repeat(MAX_CURSOR_LENGTH + 1)),
            Err(OpaqueCursorError::TooLong)
        );
        assert!(serde_json::from_str::<OpaqueCursor>(r#""bad\ncursor""#).is_err());
    }

    #[test]
    fn page_requests_apply_defaults_and_validate_bounds() {
        let request = serde_json::from_str::<CursorPageRequest>("{}").unwrap();
        assert_eq!(request.limit.get(), DEFAULT_PAGE_LIMIT);
        assert!(request.cursor.is_none());

        assert_eq!(PageLimit::new(0), Err(PageLimitError::Zero));
        assert_eq!(
            PageLimit::new(MAX_PAGE_LIMIT + 1),
            Err(PageLimitError::TooLarge)
        );
        assert!(serde_json::from_str::<CursorPageRequest>(r#"{"limit":0}"#).is_err());
        assert!(serde_json::from_str::<CursorPageRequest>(r#"{"limit":5,"offset":10}"#).is_err());
    }
}

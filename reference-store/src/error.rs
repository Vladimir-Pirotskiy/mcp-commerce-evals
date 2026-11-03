//! The ways a tool call is refused, each with the recovery hint the assistant reads.

use std::fmt;

use rmcp::model::{ContentBlock, IntoContents};

/// Longest id or other free text echoed back inside an error message.
const ECHO_CHARS: usize = 40;

/// Why a tool call was refused. Every variant renders as one message that tells the assistant
/// what to change; the MCP layer reports it as a tool result with `isError: true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// `items` was empty or longer than `max`.
    ItemCount {
        /// Most items one call may carry.
        max: usize,
    },
    /// A quantity was outside the allowed range.
    Quantity {
        /// The part the quantity was given for.
        id: String,
        /// Largest quantity per part.
        max: u32,
    },
    /// The same part, merged across lines, exceeds the per-part maximum.
    MergedQuantity {
        /// The part requested too often.
        id: String,
        /// The merged quantity.
        total: u32,
        /// Largest quantity per part.
        max: u32,
    },
    /// A text input is longer than its limit.
    TooLong {
        /// Name of the argument.
        field: &'static str,
        /// Longest accepted length in characters.
        max: usize,
    },
    /// `fits` has more entries than allowed.
    TooManyFits {
        /// Most entries.
        max: usize,
    },
    /// No part has this id.
    UnknownPart {
        /// The id as sent.
        id: String,
    },
    /// The cart cannot be created; each entry says why for one line.
    CartRefused {
        /// One sentence per problem line.
        problems: Vec<String>,
    },
}

/// `text` cut to [`ECHO_CHARS`] characters, so a hostile input cannot inflate a message.
pub fn echo(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(ECHO_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}...")
    } else {
        head
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ItemCount { max } => write!(f, "Send between 1 and {max} items."),
            Self::Quantity { id, max } => {
                write!(f, "Quantity for {} must be between 1 and {max}.", echo(id))
            }
            Self::MergedQuantity { id, total, max } => write!(
                f,
                "{} is requested {total} times in total; the most per part is {max}. Send one \
                 line with a smaller quantity.",
                echo(id)
            ),
            Self::TooLong { field, max } => {
                write!(f, "{field} is longer than {max} characters.")
            }
            Self::TooManyFits { max } => write!(f, "fits takes at most {max} requirements."),
            Self::UnknownPart { id } => write!(
                f,
                "No part has the id '{}'. Copy ids from search_products results.",
                echo(id)
            ),
            Self::CartRefused { problems } => {
                write!(f, "Cart not created. {}", problems.join(" "))
            }
        }
    }
}

impl std::error::Error for ToolError {}

impl IntoContents for ToolError {
    fn into_contents(self) -> Vec<ContentBlock> {
        self.to_string().into_contents()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_keeps_short_text_and_cuts_long_text() {
        assert_eq!(echo("ABC-1"), "ABC-1");
        let cut = echo(&"x".repeat(500));
        assert_eq!(cut.chars().count(), ECHO_CHARS + 3);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn a_message_never_echoes_a_whole_hostile_id() {
        let id = "z".repeat(10_000);
        let message = ToolError::UnknownPart { id }.to_string();
        assert!(message.len() < 200, "{}", message.len());
    }
}

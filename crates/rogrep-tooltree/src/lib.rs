//! Tool-call classification and facet-token extraction.
//!
//! Facet tokens are raw `key:value` strings indexed on the `turn_facets`
//! field and queryable via the grammar (`tool:bash`, `tool_status:failed`,
//! `git_pr_num:48`). The git/shell extraction lives in `shell.rs`/`git.rs`.

pub mod facets;
pub mod git;
pub mod gitops;
pub mod shell;

pub use facets::{facet_tokens_for_turn, output_facet_tokens};
pub use gitops::{git_ops_for_conversation, GitOp};

use super::*;
use crate::local_pool::error::ErrorCode;
use std::cell::Cell;

#[tokio::test]
async fn opencode_failure_keeps_the_codex_restart_requirement() {
    let results = refresh_client_catalogs(
        async { Ok(()) },
        async {
            Err(LocalPoolError::new(
                ErrorCode::Io,
                "synthetic OpenCode error",
            ))
        },
        async { Ok(CodexCatalogRefreshStatus::Deferred) },
    )
    .await;
    assert!(matches!(
        results.codex,
        Ok(CodexCatalogRefreshStatus::Deferred)
    ));
    assert_eq!(results.opencode.unwrap_err().code, ErrorCode::Io);
    assert!(results.claude_desktop.is_ok());
}

#[tokio::test]
async fn every_client_is_attempted_and_retains_its_own_result() {
    for claude_fails in [false, true] {
        for opencode_fails in [false, true] {
            for codex in [
                Ok(CodexCatalogRefreshStatus::Updated),
                Ok(CodexCatalogRefreshStatus::Skipped),
                Ok(CodexCatalogRefreshStatus::Deferred),
                Err(ErrorCode::GatewayUnavailable),
            ] {
                let visited = Cell::new(0u8);
                let results = refresh_client_catalogs(
                    async {
                        visited.set(visited.get() | 1);
                        if claude_fails {
                            Err(LocalPoolError::new(ErrorCode::Io, "synthetic Claude error"))
                        } else {
                            Ok(())
                        }
                    },
                    async {
                        visited.set(visited.get() | 2);
                        if opencode_fails {
                            Err(LocalPoolError::new(
                                ErrorCode::InvalidState,
                                "synthetic OpenCode error",
                            ))
                        } else {
                            Ok(())
                        }
                    },
                    async {
                        visited.set(visited.get() | 4);
                        codex.map_err(|code| LocalPoolError::new(code, "synthetic Codex error"))
                    },
                )
                .await;
                assert_eq!(visited.get(), 7);
                assert_eq!(
                    results.claude_desktop.err().map(|error| error.code),
                    claude_fails.then_some(ErrorCode::Io)
                );
                assert_eq!(
                    results.opencode.err().map(|error| error.code),
                    opencode_fails.then_some(ErrorCode::InvalidState)
                );
                assert_eq!(results.codex.map_err(|error| error.code), codex);
            }
        }
    }
}

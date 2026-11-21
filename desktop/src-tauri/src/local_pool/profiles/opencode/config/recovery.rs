use super::{DesktopState, LocalPoolError, Path};

mod plan;

pub(in crate::local_pool::profiles::opencode) fn restore_saved_config(
    state: &DesktopState,
    path: &Path,
) -> Result<bool, LocalPoolError> {
    let Some(prepared) = plan::prepare(state, path)? else {
        return Ok(false);
    };
    prepared.apply()?;
    Ok(true)
}

#[cfg(test)]
mod tests;

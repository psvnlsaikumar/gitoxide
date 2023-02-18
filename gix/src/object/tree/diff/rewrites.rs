use crate::config::cache::util::ApplyLeniency;
use crate::config::tree::Diff;
use crate::diff::rename::Tracking;
use crate::object::tree::diff::Rewrites;

/// From where to source copies
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum CopySource {
    /// Find copies from the set of changed files only.
    FromSetOfChangedFiles,
}

/// How to determine copied files.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Copies {
    /// The set of files to search when finding the source of copies.
    pub source: CopySource,
    /// Equivalent to [`Rewrites::percentage`], but used for copy tracking.
    ///
    /// Useful to have similarity-based rename tracking and cheaper copy tracking, which also is the default
    /// as only identity plays a role.
    pub percentage: Option<f32>,
}

/// The error returned by [`Rewrites::try_from_config()].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    DiffRenames(#[from] crate::config::key::GenericError),
    #[error(transparent)]
    DiffRenameLimit(#[from] crate::config::unsigned_integer::Error),
}

/// The default settings for rewrites according to the git configuration defaults.
impl Default for Rewrites {
    fn default() -> Self {
        Rewrites {
            copies: None,
            percentage: Some(0.5),
            limit: 1000,
        }
    }
}

impl Rewrites {
    /// Create an instance by reading all relevant information from the `config`uration, while being `lenient` or not.
    /// Returns `Ok(None)` if nothing is configured.
    ///
    /// Note that missing values will be defaulted similar to what git does.
    #[allow(clippy::result_large_err)]
    pub fn try_from_config(config: &gix_config::File<'static>, lenient: bool) -> Result<Option<Self>, Error> {
        let key = "diff.renames";
        let copies = match config
            .boolean_by_key(key)
            .map(|value| Diff::RENAMES.try_into_renames(value, || config.string_by_key(key)))
            .transpose()
            .with_leniency(lenient)?
        {
            Some(renames) => match renames {
                Tracking::Disabled => return Ok(None),
                Tracking::Renames => None,
                Tracking::RenamesAndCopies => Some(Copies {
                    source: CopySource::FromSetOfChangedFiles,
                    percentage: None,
                }),
            },
            None => return Ok(None),
        };

        let default = Self::default();
        Ok(Rewrites {
            copies,
            limit: config
                .integer_by_key("diff.renameLimit")
                .map(|value| Diff::RENAME_LIMIT.try_into_usize(value))
                .transpose()
                .with_leniency(lenient)?
                .unwrap_or(default.limit),
            ..default
        }
        .into())
    }
}
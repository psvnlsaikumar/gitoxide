use crate::bstr::BStr;
use crate::ext::ObjectIdExt;
use crate::object::tree::diff::Rewrites;
use crate::Repository;
use gix_diff::tree::visit::Change;
use std::ops::Range;

/// A set of tracked items allows to figure out their relations by figuring out their similarity.
struct Item {
    /// The underlying raw change
    change: Change,
    /// That slice into the backing for paths.
    location: Range<usize>,
    /// If true, this item was already emitted, i.e. seen by the caller.
    emitted: bool,
}

impl Item {
    fn location<'a>(&self, backing: &'a [u8]) -> &'a BStr {
        backing[self.location.clone()].as_ref()
    }
}

pub struct State {
    items: Vec<Item>,
    path_backing: Vec<u8>,
    rewrites: Rewrites,
}

pub mod visit {
    use crate::bstr::BStr;
    use gix_diff::tree::visit::Change;

    pub struct Source<'a> {
        pub mode: gix_object::tree::EntryMode,
        pub id: gix_hash::ObjectId,
        pub kind: Kind,
        pub location: &'a BStr,
    }

    pub enum Kind {
        RenameTarget,
    }

    impl Kind {
        pub fn can_use_change(&self, change: &Change) -> bool {
            match self {
                Kind::RenameTarget => matches!(change, Change::Deletion { .. }),
            }
        }
    }

    pub struct Destination<'a> {
        pub change: gix_diff::tree::visit::Change,
        pub location: &'a BStr,
    }
}

impl State {
    pub fn new(renames: Rewrites) -> Self {
        State {
            items: vec![],
            path_backing: vec![],
            rewrites: renames,
        }
    }
}

/// internal
impl State {
    /// Find `item` in our set of items ignoring `item_idx` to avoid finding ourselves, by similarity indicated by `percentage`.
    /// The latter can be `None` or `Some(x)` where `x>=1` for identity, and anything else for similarity.
    /// We also ignore emitted items entirely.
    /// Use `kind` to indicate what kind of match we are looking for, which might be deletions matching an `item` addition, or
    /// any non-deletion otherwise.
    /// Note that we always try to find by identity first even if a percentage is given as it's much faster and may reduce the set
    /// of items to be searched.
    fn find_match(
        &self,
        item: &Item,
        item_idx: usize,
        percentage: Option<f32>,
        kind: visit::Kind,
        repo: &Repository,
    ) -> Result<Option<(usize, &Item)>, crate::object::tree::diff::for_each::Error> {
        let item_id = item.change.oid();
        for similarity in [None, percentage] {
            if needs_exact_match(similarity) {
                let first_idx = self.items.partition_point(|a| a.change.oid() < item_id);
                let range = match self.items.get(first_idx..).map(|items| {
                    let end = items
                        .iter()
                        .position(|a| a.change.oid() != item_id)
                        .map(|idx| first_idx + idx)
                        .unwrap_or(self.items.len());
                    first_idx..end
                }) {
                    Some(range) => range,
                    None => return Ok(None),
                };
                if range.is_empty() {
                    return Ok(None);
                }
                let res = self.items[range.clone()].iter().enumerate().find(|(src_idx, src)| {
                    *src_idx + range.start != item_idx && !src.emitted && kind.can_use_change(&src.change)
                });
                if let Some(src) = res {
                    return Ok(Some(src));
                }
            } else {
                let mut object = item_id.to_owned().attach(repo).object()?;
                let percentage = percentage.expect("it's set to something below 1.0 and we assured this");
                debug_assert!(
                    item.change.oid_and_mode().1.is_blob(),
                    "symlinks are matched exactly, and trees aren't used here"
                );
                for (can_idx, src) in self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(idx, item)| *idx != item_idx && !item.emitted && kind.can_use_change(&item.change))
                {
                    debug_assert!(src.change.oid_and_mode().1.is_blob());
                    let src_obj = src.change.oid().to_owned().attach(repo).object()?;
                    let platform = crate::object::blob::diff::Platform {
                        old: src_obj,
                        new: object,
                        algo: repo.config.diff_algorithm()?,
                    };
                    let tokens = platform.line_tokens();
                    let counts =
                        gix_diff::blob::diff(platform.algo, &tokens, crate::diff::blob::sink::Counter::default());
                    let similarity = (tokens.before.len() - counts.removals as usize) as f32
                        / tokens.before.len().max(tokens.after.len()) as f32;
                    dbg!(similarity, percentage);
                    object = platform.new;
                    if similarity >= percentage {
                        return Ok(Some((can_idx, src)));
                    }
                }
            }
            if percentage.is_none() {
                break;
            }
        }
        Ok(None)
    }
}

/// build state and find matches.
impl State {
    /// We may refuse the push if that information isn't needed for what we have to track.
    pub fn try_push_change(&mut self, change: Change, location: &BStr) -> Option<Change> {
        if !change.oid_and_mode().1.is_blob() {
            return Some(change);
        }
        let keep = match (self.rewrites.copies, &change) {
            (Some(_find_copies), _) => true,
            (None, Change::Modification { .. }) => false,
            (None, _) => true,
        };

        if !keep {
            return Some(change);
        }

        let start = self.path_backing.len();
        self.path_backing.extend_from_slice(location);
        self.items.push(Item {
            location: start..self.path_backing.len(),
            change,
            emitted: false,
        });
        None
    }

    /// Can only be called once effectively as it alters its own state.
    ///
    /// `cb(destination, source)` is called for each item, either with `Some(source)` if it's
    /// the destination of a copy or rename, or with `None` for source if no relation to other
    /// items in the tracked set exist.
    pub fn emit(
        &mut self,
        mut cb: impl FnMut(visit::Destination<'_>, Option<visit::Source<'_>>) -> gix_diff::tree::visit::Action,
        repo: &Repository,
    ) -> Result<(), crate::object::tree::diff::for_each::Error> {
        self.items.sort_by(|a, b| {
            a.change.oid().cmp(b.change.oid()).then_with(|| {
                a.location
                    .start
                    .cmp(&b.location.start)
                    .then(a.location.end.cmp(&b.location.end))
            })
        });

        if self.find_renames(&mut cb, repo)? == gix_diff::tree::visit::Action::Cancel {
            return Ok(());
        }
        if let Some(_copies) = self.rewrites.copies {
            todo!("copy tracking")
        }

        for item in self.items.drain(..).filter(|item| !item.emitted) {
            if cb(
                visit::Destination {
                    location: item.location(&self.path_backing),
                    change: item.change,
                },
                None,
            ) == gix_diff::tree::visit::Action::Cancel
            {
                break;
            }
        }
        Ok(())
    }

    fn find_renames(
        &mut self,
        cb: &mut impl FnMut(visit::Destination<'_>, Option<visit::Source<'_>>) -> gix_diff::tree::visit::Action,
        repo: &Repository,
    ) -> Result<gix_diff::tree::visit::Action, crate::object::tree::diff::for_each::Error> {
        // TODO(perf): reuse object data and interner state and interned tokens, make these available to `find_match()`
        let mut dest_ofs = 0;
        while let Some((mut dest_idx, dest)) = self.items[dest_ofs..].iter().enumerate().find_map(|(idx, item)| {
            (!item.emitted && matches!(item.change, Change::Addition { .. })).then_some((idx, item))
        }) {
            dest_idx += dest_ofs;
            dest_ofs = dest_idx + 1;
            let src = self
                .find_match(
                    dest,
                    dest_idx,
                    self.rewrites.percentage,
                    visit::Kind::RenameTarget,
                    repo,
                )?
                .map(|(src_idx, src)| {
                    let (id, mode) = src.change.oid_and_mode();
                    let id = id.to_owned();
                    let location = src.location(&self.path_backing);
                    (
                        visit::Source {
                            mode,
                            id,
                            kind: visit::Kind::RenameTarget,
                            location,
                        },
                        src_idx,
                    )
                });
            let location = dest.location(&self.path_backing);
            let change = dest.change.clone();
            let dest = visit::Destination { change, location };
            self.items[dest_idx].emitted = true;
            if let Some(src_idx) = src.as_ref().map(|t| t.1) {
                self.items[src_idx].emitted = true;
            }
            if cb(dest, src.map(|t| t.0)) == gix_diff::tree::visit::Action::Cancel {
                return Ok(gix_diff::tree::visit::Action::Cancel);
            }
        }
        Ok(gix_diff::tree::visit::Action::Continue)
    }
}

fn needs_exact_match(percentage: Option<f32>) -> bool {
    percentage.map_or(true, |p| p >= 1.0)
}

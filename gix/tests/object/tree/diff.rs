use std::convert::Infallible;

use gix_object::{bstr::ByteSlice, tree::EntryMode};

use gix::object::{blob::diff::line::Change, tree::diff::change::Event};

use crate::named_repo;

#[test]
fn changes_against_tree_modified() -> crate::Result {
    let repo = named_repo("make_diff_repo.sh")?;
    let from = tree_named(&repo, "@^{/c3}~1");
    let to = tree_named(&repo, ":/c3");
    from.changes()?
        .for_each_to_obtain_tree(&to, |change| -> Result<_, Infallible> {
            assert_eq!(change.location, "", "without configuration the location field is empty");
            match change.event {
                Event::Modification {
                    previous_entry_mode,
                    previous_id,
                    entry_mode,
                    id,
                } => {
                    assert_eq!(previous_entry_mode, EntryMode::Blob);
                    assert_eq!(entry_mode, EntryMode::Blob);
                    assert_eq!(previous_id.object().unwrap().data.as_bstr(), "a\n");
                    assert_eq!(id.object().unwrap().data.as_bstr(), "a\na1\n");
                }
                Event::Rewrite { .. } | Event::Deletion { .. } | Event::Addition { .. } => {
                    unreachable!("only modification is expected")
                }
            };

            let diff = change.event.diff().expect("changed file").expect("objects available");
            let count = diff.line_counts();
            assert_eq!(count.insertions, 1);
            assert_eq!(count.removals, 0);
            diff.lines(|hunk| {
                match hunk {
                    Change::Deletion { .. } => unreachable!("there was no deletion"),
                    Change::Addition { lines } => assert_eq!(lines, vec!["a1".as_bytes().as_bstr()]),
                    Change::Modification { .. } => unreachable!("there was no modification"),
                };
                Ok::<_, Infallible>(())
            })
            .expect("infallible");
            Ok(Default::default())
        })?;
    Ok(())
}

#[test]
fn changes_against_tree_with_filename_tracking() -> crate::Result {
    let repo = named_repo("make_diff_repo.sh")?;
    let from = repo.empty_tree();
    let to = tree_named(&repo, ":/c1");

    let mut expected = vec!["a", "b", "c", "d"];
    from.changes()?
        .track_filename()
        .for_each_to_obtain_tree(&to, |change| -> Result<_, Infallible> {
            expected.retain(|name| name != change.location);
            Ok(Default::default())
        })?;
    assert_eq!(expected, Vec::<&str>::new(), "all paths should have been seen");

    let mut expected = vec!["a", "b", "dir/c", "d"];
    from.changes()?
        .track_path()
        .for_each_to_obtain_tree(&to, |change| -> Result<_, Infallible> {
            expected.retain(|name| name != change.location);
            Ok(Default::default())
        })?;
    assert_eq!(expected, Vec::<&str>::new(), "all paths should have been seen");

    let err = from
        .changes()?
        .track_path()
        .for_each_to_obtain_tree(&to, |_change| {
            Err(std::io::Error::new(std::io::ErrorKind::Other, "custom error"))
        })
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "The user-provided callback failed",
        "custom errors made visible and not squelched"
    );
    Ok(())
}

fn tree_named(repo: &gix::Repository, rev_spec: impl AsRef<str>) -> gix::Tree {
    repo.rev_parse_single(rev_spec.as_ref())
        .unwrap()
        .object()
        .unwrap()
        .peel_to_kind(gix::object::Kind::Tree)
        .unwrap()
        .into_tree()
}

mod rewrites {
    use crate::object::tree::diff::tree_named;
    use crate::util::named_repo;
    use gix::object::tree::diff::change::Event;
    use gix::object::tree::diff::Rewrites;
    use gix_ref::bstr::BStr;
    use std::convert::Infallible;

    #[test]
    #[ignore]
    fn identity_with_mixed_in_modifications() -> crate::Result {
        Ok(())
    }

    #[test]
    fn rename_identity() -> crate::Result {
        let repo = named_repo("make_diff_repo.sh")?;
        for (commit_msg, expected, assert_msg) in [
            (
                "r1-identity",
                vec![BStr::new("a"), "dir/a-moved".into()],
                "one rename and nothing else",
            ),
            (
                "r2-ambiguous",
                vec![
                    "s1".into(),
                    "b1".into(),
                    "s2".into(),
                    "b2".into(),
                    "s3".into(),
                    "z".into(),
                ],
                "multiple possible sources decide by ordering everything lexicographically",
            ),
            (
                "c4 - add identical files",
                vec![],
                "not having any renames is OK as well",
            ),
        ] {
            let from = tree_named(&repo, format!("@^{{/{commit_msg}}}~1"));
            let to = tree_named(&repo, format!(":/{commit_msg}"));

            for percentage in [None, Some(0.5)] {
                let mut actual = Vec::new();
                from.changes()?
                    .track_path()
                    .track_rewrites(
                        Rewrites {
                            percentage,
                            ..Default::default()
                        }
                        .into(),
                    )
                    .for_each_to_obtain_tree(&to, |change| -> Result<_, Infallible> {
                        if !change.event.entry_mode().is_tree() {
                            if let Event::Rewrite { source_location, .. } = change.event {
                                actual.push(source_location.to_owned());
                                actual.push(change.location.to_owned());
                            }
                        }
                        Ok(Default::default())
                    })?;
                assert_eq!(actual, expected, "{assert_msg}")
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "fails because our similarity computation isn't exactly as the one from git"]
    fn rename_fuzzy() -> crate::Result {
        let repo = named_repo("make_diff_repo.sh")?;
        let from = tree_named(&repo, "@^{/r3}~1");
        let to = tree_named(&repo, ":/r3");

        for percentage in [
            None,
            Some(0.76), /*cutoff point where git stops seeing it as equal */
        ] {
            let mut actual = Vec::new();
            let mut rewrite_count = 0;
            from.changes()?
                .track_path()
                .track_rewrites(
                    Rewrites {
                        percentage,
                        ..Default::default()
                    }
                    .into(),
                )
                .for_each_to_obtain_tree(&to, |change| -> Result<_, Infallible> {
                    if !change.event.entry_mode().is_tree() {
                        if let Event::Rewrite { .. } = change.event {
                            rewrite_count += 0;
                        } else {
                            actual.push(change.location.to_owned());
                        }
                    }
                    Ok(Default::default())
                })?;
            assert_eq!(
                actual,
                vec![BStr::new("b"), "dir/c-moved".into(), "dir/c".into()],
                "these items include no rewrite as the cut-off is chosen accordingly"
            )
        }

        let mut actual = Vec::new();
        from.changes()?
            .track_path()
            .track_rewrites(
                Rewrites {
                    percentage: Some(0.75),
                    ..Default::default()
                }
                .into(),
            )
            .for_each_to_obtain_tree(&to, |change| -> Result<_, Infallible> {
                if !change.event.entry_mode().is_tree() {
                    if let Event::Rewrite { source_location, .. } = change.event {
                        actual.push(source_location.to_owned());
                        actual.push(change.location.to_owned());
                    }
                }
                Ok(Default::default())
            })?;
        assert_eq!(
            actual,
            vec![BStr::new("dir/c"), "dir/c-moved".into()],
            "it found all items at the cut-off point, similar to git"
        );

        Ok(())
    }
}

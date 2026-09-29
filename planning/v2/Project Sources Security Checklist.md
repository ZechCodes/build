# Project sources security checklist

Scope: every client-named Git remote and source folder a project verb hands to
git or writes into a project (#228): `project.create`, `project.add_source`,
`project.update_source`, `project.set_remote` and `workspace.add_directory`.
A remote reaches git as a process argument, never through a shell, so the
threat is git reading the text as something other than a location.

| Control | Evidence |
| --- | --- |
| Git is never run through a shell; every remote and path is its own argument. | `bridge/src/lifecycle/projects.rs` and `bridge/src/lifecycle/source_update.rs` spawn `git` with `Command` args only; `git clone`, `git remote add` and `git remote set-url` get `--` before the positional remote. |
| A remote git would read as an option (`--upload-pack=…`, `-oProxyCommand=…`, `--mirror=…`) is refused on the bridge, before git runs. | `a_remote_git_would_read_as_an_option_is_refused`, `add_source_refuses_a_remote_git_would_read_as_an_option`, `set_remote_refuses_a_remote_git_would_read_as_an_option`, `add_directory_refuses_a_remote_git_would_read_as_an_option`, `a_hostile_remote_is_refused_before_git_sees_it`. Each fails without the check: git ran `--upload-pack` and `--mirror` as options. |
| An ssh user, host or port git would hand to `ssh` as an option (`ssh://-oProxyCommand=sh/x`, `git@-oProxyCommand=sh:x`, a dash after `user@`) is refused by Build's own check, not left to git's. Git percent-decodes an ssh url's user, host and port first, so any `%` there (`ssh://%2DoProxyCommand=sh/x`) is refused too. | `an_ssh_host_user_or_port_git_would_read_as_an_option_is_refused`, `an_ssh_authority_git_would_percent_decode_is_refused`. |
| A transport helper that runs a program (`ext::`, `fd::`) is refused. | `a_transport_helper_that_runs_a_program_is_refused`, `project_create_refuses_a_hostile_remote_in_its_sources_or_its_origin`. |
| A second argument or config line cannot be smuggled in (whitespace, newline, NUL, control characters). | `a_second_argument_or_line_cannot_be_smuggled_in`. |
| Only https, http, ssh, git and file urls, `user@host:path` and absolute paths are accepted; the length is capped. | `the_remotes_people_clone_from_are_accepted_trimmed`, `a_remote_that_names_nowhere_git_should_go_is_refused`. |
| The check is also inside the git mutations, so a caller that skips the verb-level check still cannot pass an option. | `the_git_that_takes_a_remote_refuses_one_that_reads_as_an_option`. |
| A source folder named by a client must exist, be a directory, be canonical, and not hold or sit inside any registered source, whether added or moved. | `usable_source_move` and `source_request` both go through `canonical_source_path` and the overlap check; `a_later_source_moves_to_another_folder_held_to_the_same_checks_as_adding_one`. |
| The project's home folder (first source) cannot be moved by an edit. | `a_later_source_moves_to_another_folder_held_to_the_same_checks_as_adding_one`. |
| A base branch must be a valid branch name, not an option or revision expression, and must exist in the checkout. | `a_base_branch_must_be_a_branch_the_checkout_has`, `a_base_branch_is_one_the_checkout_has_and_the_first_sources_is_the_projects`. |
| A refused edit writes nothing: every part is proved before the remote is written. | `a_refused_base_leaves_the_remote_unwritten`. |
| A remote change reaches only checkouts cut from that source. A workspace `origin` changed by hand is left alone, and a symlinked `.git` is not followed. | `a_remote_moves_the_checkout_and_the_copies_that_still_named_the_old_one`, `a_checkout_whose_git_directory_is_a_symlink_is_not_followed`, `a_remote_moves_existing_workspace_copies_that_still_named_the_old_one`. |
| A workspace checkout git will not rewrite is never passed off as moved: its `origin` is read back after every write, and the answer's `checkouts_failed` names it (workspace, path, git's reason) so the SPA can say it is still on the old remote. Nothing is rolled back. | `a_checkout_git_cannot_rewrite_is_reported_and_the_rest_still_move`, `a_checkout_that_keeps_its_remote_when_it_is_taken_off_is_reported`, `a_workspace_copy_git_cannot_rewrite_is_named_in_the_answer`; `names the workspaces a remote change could not reach` (`spa/test/projectSettings.test.js`). |
| A source label is one bounded line with no control characters. | `an_edit_names_a_real_source_and_something_to_change`. |
| Everything a source row carries is escaped when the SPA paints it. | `escapes what the project record carries` (`spa/test/projectSettings.test.js`); the cards render every value through `esc`. |

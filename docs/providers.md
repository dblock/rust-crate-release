# Release Providers

## Git Repositories

Construct `Git::new(root, default_branch, remote)` and use it through `ReleaseRepository`. The directory must be the canonical Git root; the configured remote is a name, not a URL or arbitrary command option. `inspect` captures the current default-branch head/tree, remote head, and source timestamp; callers separately enforce clean state and the expected concurrency relationship.

Canonical paths use `dunce` to retain normal Windows drive/UNC syntax where safe. Rust's verbatim `\\?\` paths are valid filesystem inputs but can break Git's bundle locking and Cargo's Git-backed package discovery; filesystem identity and containment checks remain canonical.

Trusted preparation starts from a clean checkout. `stage_changes` enumerates changed and untracked paths, stages literal names rather than glob-expanded pathspecs, and returns the index tree. After validation, `assert_prepared(tree)` rejects index, tracked, or untracked changes. `commit` uses only that exact tree and reuses an existing matching HEAD rather than making an empty duplicate. If a remote descendant already contains precisely the staged tree, a compare-and-swap fast-forward adopts that matching commit without rewriting source or creating equivalent divergent history.

`ensure_tag` reuses matching local tags and rejects conflicts without moving them. `preserve_bundle` writes an immutable recovery bundle containing the matching release tag. The default artifact directory is inside Git's private metadata (`git rev-parse --git-path rcr`), so retained diagnostics do not dirty consumer projects or require modifying their ignore files.

`push_release` checks current remote refs, atomically pushes the release branch/tag, then reads them back. A remote descendant already containing the release is preserved rather than rewound; conflicting tags or divergent concurrent history fail. `push_development` likewise reuses matching/descendant output and refuses divergent updates. No method force-pushes, resets, discards user edits, or authorizes retrying a registry upload.

## Semantic History

`GitHistory::new(git)` implements `SemanticHistory`. It rejects shallow repositories, selects the highest numerically reachable stable `v<version>` tag, and returns full commit messages after that baseline in chronological history order. Prerelease/build-metadata tags and noncanonical tag formats do not select a baseline. Discovery binds to an explicit complete source object ID rather than whichever HEAD exists later.

## Isolated Consumers

`testing::GitProject::new(files)` creates a disposable real working repository and bare `origin`, with a committed default branch. `cargo_library()` supplies a small dependency-free Cargo project with matching manifest/lockfile and changelog. Fixtures disable signing, inherited hooks, and automatic newline conversion through local configuration; they do not change global Git settings.

`copy_project(source, destination)` recursively includes hidden files but skips Git metadata and its platform aliases, without following symbolic links. `copy_project_selected` accepts `CopySelection::Glob` with portable forward-slash patterns or `CopySelection::Paths` containing exact relative files. Globs include hidden files; arrays retain literal bracket/star semantics, and duplicate selections are deduplicated. Selected files must remain inside their canonical source root. Copying refuses overlapping roots, metadata selections, links, and overwriting existing destination files.

`LocalRegistry` implements the real publisher interface by retaining validated archives in a local directory, refusing version/checksum conflicts, and reusing identical bytes. `DirectoryReleaseHost` retains captured metadata and the exact archive under the release tag; matching output is reused and differing owned metadata/assets are replaced. Both expose mutation counts for behavioral assertions and are selected only by explicit injection.

The helpers are framework-independent and do not sandbox trusted callbacks or replace production publication with a simulation implicitly. Release tests must inject explicit local registry/hosting adapters; no fixture helper authorizes network publication or credential setup.

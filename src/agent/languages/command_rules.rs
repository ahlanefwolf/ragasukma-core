//! Rule tables for classifying shell commands as verification vs. noise.
//!
//! Everything here is data, not logic: [`super::is_verification_command`] decides
//! *order*, this module decides *content*. Keeping them apart means adding a new
//! toolchain is a one-line table edit instead of a surgery in the matcher.
//!
//! Prefix tables are matched with [`matches_prefix`] (plain `starts_with` semantics),
//! so entries that need a word boundary keep their trailing space.

/// Returns `true` when `sub` starts with any of `prefixes`.
pub(crate) fn matches_prefix(sub: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| sub.starts_with(p))
}

/// Splits a shell command into sub-commands on `&`, `;` and `|`, trimming each part.
pub(crate) fn split_chained_commands(lower: &str) -> impl Iterator<Item = &str> {
    lower
        .split(['&', ';', '|'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

/// Trims surrounding parentheses and whitespace from a sub-command.
pub(crate) fn trim_sub_command(sub: &str) -> &str {
    sub.trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace())
}

/// Commands that only rearrange the filesystem. They cannot fail in a way that says
/// anything about the code's correctness, so a successful run is not evidence.
///
/// This is deliberately a property ("did the command merely create, move, or delete
/// files?") rather than an exhaustive inventory of every shell builtin that can do so.
pub(crate) const FS_MUTATION_PREFIXES: &[&str] = &[
    "mkdir",
    "md ",
    "new-item",
    "ni ",
    "touch",
    "cp ",
    "copy ",
    "copy-item",
    "mv ",
    "move ",
    "move-item",
    "rm ",
    "del ",
    "remove-item",
    "rd ",
    "rmdir",
    "set-content",
    "add-content",
    "out-file",
    "chmod",
    "chown",
    "ln ",
];

/// A bare existence probe reports a fact about the filesystem, not about behaviour.
///
/// Entries are deliberately *narrow*: `"ls "` needs its trailing space so a real
/// verifier whose name merely starts with those letters (e.g. `lsof`-style tools,
/// or a wrapper named `ls-tests`) is not swallowed by the probe filter.
pub(crate) const FS_PROBE_PREFIXES: &[&str] =
    &["test-path", "test ", "[ -f", "[ -d", "ls ", "dir "];

/// Returns `true` when `sub` is a bare existence/listing probe with no verification intent.
///
/// The probe table and the argument scan disagree about words like `Path` and `Directory`:
/// `Test-Path src` is a probe whose tail argument would otherwise match a verification
/// keyword, so skipping purely on the keyword is wrong. A probe is only "bare" when the
/// command itself is a probe AND carries none of the unambiguous verification
/// substrings — a probe piped into `pytest` still counts as evidence.
pub(crate) fn is_probe_only(sub: &str) -> bool {
    if !matches_prefix(sub, FS_PROBE_PREFIXES) {
        return false;
    }
    let has_verifier = KNOWN_VERIFICATION_PREFIXES.iter().any(|v| sub.contains(v))
        || VERIFY_ARG_SUBSTRINGS.iter().any(|kw| sub.contains(kw));
    !has_verifier
}

/// Commands that are purely informational / navigation and NEVER count as verification.
///
/// `ls` and `dir` appear here as well as in [`FS_PROBE_PREFIXES`] on purpose: both
/// tables mean "no signal", and being listed twice keeps each table readable on its own.
pub(crate) const PURE_INFO_PREFIXES: &[&str] = &[
    "git status",
    "git log",
    "git diff",
    "git show",
    "git branch",
    "git remote",
    "ls ",
    "ls",
    "dir",
    "pwd",
    "cd ",
    "echo ",
    "cat ",
    "type ",
    "head ",
    "tail ",
    "find ",
    "which ",
    "where ",
    "whoami",
    "hostname",
    "uname",
];

/// Package installation / dependency management commands that MUST NOT count as verification.
pub(crate) const PKG_MGMT_PREFIXES: &[&str] = &[
    // JavaScript / TypeScript
    "npm install",
    "npm i ",
    "npm i",
    "npm add",
    "npm update",
    "npm remove",
    "npm uninstall",
    "npm list",
    "npm audit",
    "npm init",
    "pnpm install",
    "pnpm i ",
    "pnpm i",
    "pnpm add",
    "pnpm update",
    "pnpm remove",
    "pnpm list",
    "pnpm audit",
    "yarn add",
    "yarn install",
    "yarn remove",
    "yarn list",
    "yarn why",
    "yarn init",
    "bun install",
    "bun i ",
    "bun i",
    "bun add",
    "bun remove",
    "bun init",
    // Rust
    "cargo add",
    "cargo install",
    "cargo remove",
    "cargo update",
    "cargo tree",
    "cargo new",
    "cargo init",
    "cargo metadata",
    "cargo run",
    // Python
    "pip install",
    "pip uninstall",
    "pip list",
    "pip freeze",
    "pip3 install",
    "pip3 uninstall",
    "pip3 list",
    "pip3 freeze",
    "poetry add",
    "poetry install",
    "poetry remove",
    "poetry lock",
    "poetry show",
    "uv add",
    "uv pip",
    "uv sync",
    "uv lock",
    // PHP
    "composer install",
    "composer require",
    "composer update",
    "composer remove",
    // Go
    "go get",
    "go install",
    "go mod",
];

/// Explicit, curated list of verification entry points across toolchains.
pub(crate) const KNOWN_VERIFICATION_PREFIXES: &[&str] = &[
    "cargo test",
    "cargo check",
    "cargo build",
    "cargo clippy",
    "cargo fmt --check",
    "rustc",
    "npm test",
    "npm run test",
    "npm run build",
    "npm run typecheck",
    "npm run lint",
    "npm run check",
    "pnpm test",
    "pnpm run test",
    "pnpm run build",
    "pnpm run typecheck",
    "pnpm run lint",
    "yarn test",
    "yarn run test",
    "yarn run build",
    "yarn run typecheck",
    "yarn run lint",
    "bun test",
    "bun run build",
    "bun run check",
    "bun run lint",
    "deno test",
    "deno check",
    "deno lint",
    "node --test",
    "node --experimental-strip-types --test",
    "node test",
    "npx tsc",
    "tsc",
    "vite build",
    "pytest",
    "python -m unittest",
    "python3 -m unittest",
    "py -m unittest",
    "python -m pytest",
    "python3 -m pytest",
    "py -m pytest",
    "uv run pytest",
    "poetry run pytest",
    "pipenv run pytest",
    "go test",
    "go vet",
    "go build",
    "flutter test",
    "flutter analyze",
    "dart test",
    "dart analyze",
    "dotnet test",
    "dotnet build",
    "gradle test",
    "./gradlew test",
    "mvn test",
    "mvn verify",
    "ctest",
    "cmake --build",
    "make test",
    "make check",
    "meson test",
    "ninja",
    "php artisan test",
    "./vendor/bin/phpunit",
    "composer test",
    "./vendor/bin/pest",
    "bundle exec rspec",
    "bundle exec rake test",
    "rake test",
    "mix test",
    "swift test",
    "swift build",
    "zig build test",
    "zig build",
    "vitest",
    "jest",
    "just test",
    "just check",
    "task test",
    "task check",
    "bazel test",
    "buck test",
    "pants test",
];

/// Dedicated build & test runners only (excluding multi-purpose package managers like cargo/npm).
pub(crate) const BUILD_TOOL_BINARIES: &[&str] = &[
    "make", "ninja", "cmake", "ctest", "just", "task", "bazel", "buck", "buck2", "meson", "scons",
    "rake", "gradle", "mvn", "pytest", "vitest", "jest", "mocha", "dune", "sbt",
];

/// Substring keywords that mark the invoked *binary* itself as a verification script.
pub(crate) const SCRIPT_VERIFY_KEYWORDS: &[&str] = &["test", "check", "verify", "lint"];

/// Exact argument tokens that signal a verification invocation.
pub(crate) const VERIFY_ARGS: &[&str] = &[
    "test",
    "check",
    "lint",
    "build",
    "typecheck",
    "verify",
    "validate",
];

/// Substring argument signals (tail position args like `--run-tests` or `pytest` paths).
pub(crate) const VERIFY_ARG_SUBSTRINGS: &[&str] = &["pytest", "unittest"];

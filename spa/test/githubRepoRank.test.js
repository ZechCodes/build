// Fuzzy search over the machine's GitHub repositories: a subsequence of the
// query in `name_with_owner`, contiguous and word-start hits first.
import { expect, it } from "vitest";
import { fuzzyScore, rankRepos } from "../src/core/githubRepos.js";

const repo = (name_with_owner, pushed_at = "2026-09-01T00:00:00Z") => ({
  name_with_owner, pushed_at, ssh_url: `git@github.com:${name_with_owner}.git`, url: `https://github.com/${name_with_owner}`, private: false,
});
const names = (repos) => repos.map((item) => item.name_with_owner);

it("matches a subsequence case-insensitively and nothing else", () => {
  expect(fuzzyScore("ZB", "zech/build")).not.toBeNull();
  expect(fuzzyScore("bz", "zech/build")).toBeNull();
  expect(fuzzyScore("zech/build/more", "zech/build")).toBeNull();
});

it("ranks own/rep contiguous and word-start hits above scattered ones", () => {
  const repos = [repo("someone/owned-prep"), repo("o-w-n/r-e-p"), repo("owner/repo"), repo("xowny/xrepx")];
  expect(names(rankRepos("own/rep", repos))[0]).toBe("owner/repo");
  expect(names(rankRepos("own/rep", repos))).not.toContain(undefined);
});

it("prefers a hit at the start of the repository name over one inside a word", () => {
  const repos = [repo("zech/rebuild"), repo("zech/build")];
  expect(names(rankRepos("build", repos))).toEqual(["zech/build", "zech/rebuild"]);
});

it("lists at most eight, and an empty query lists the most recently pushed first", () => {
  const repos = Array.from({ length: 12 }, (_, index) => repo(`org/repo-${index}`, `2026-09-${String(index + 1).padStart(2, "0")}T00:00:00Z`));
  expect(rankRepos("repo", repos)).toHaveLength(8);
  expect(names(rankRepos("", repos)).slice(0, 2)).toEqual(["org/repo-11", "org/repo-10"]);
  expect(rankRepos("  ", repos)).toHaveLength(8);
});

it("breaks a tie by the most recent push, whatever trails the match", () => {
  const repos = [repo("a/app", "2026-01-01T00:00:00Z"), repo("b/app", "2026-09-01T00:00:00Z")];
  expect(names(rankRepos("app", repos))).toEqual(["b/app", "a/app"]);
  const trailing = [repo("8ly/buildkit", "2026-08-01T00:00:00Z"), repo("ZechCodes/build-web", "2026-09-01T00:00:00Z")];
  expect(names(rankRepos("build", trailing))).toEqual(["ZechCodes/build-web", "8ly/buildkit"]);
});

it("finds nothing for a typed remote URL no repository is named like", () => {
  expect(rankRepos("git@example.com:someone/else.git", [repo("zech/build")])).toEqual([]);
});

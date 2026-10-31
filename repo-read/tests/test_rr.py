"""rr.py checks: URL parsing, pattern handling, token parsing, one BAD_ARG error (one ERROR line plus its
log), and a local digest. RR_ONLINE=1 also maps and digests a small GitHub repo and a partial clone of a
large one. Run: python -m unittest discover -s repo-read/tests (in the dev env)."""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
RR = HERE.parent / "rr.py"
sys.path.insert(0, str(HERE.parent))
import rr  # noqa: E402

ONLINE = os.environ.get("RR_ONLINE") == "1"


def run_rr(args, root, extra_env=None):
    env = dict(os.environ, RR_ROOT=str(root), CLIERR_LOG_ROOT=str(Path(root) / "logs"))
    env.pop("RR_WIN_PREFIX", None)
    env.update(extra_env or {})
    return subprocess.run([sys.executable, str(RR), *args], capture_output=True, text=True,
                          encoding="utf-8", errors="replace", env=env)


class Units(unittest.TestCase):
    def test_repo_ref(self):
        r = rr.repo_ref("https://github.com/potatameister/PaperKnife/tree/main")
        self.assertEqual((r["owner"], r["repo"], r["branch"], r["sub"]), ("potatameister", "PaperKnife", "main", None))
        self.assertEqual(r["clone_url"], "https://github.com/potatameister/PaperKnife")
        r = rr.repo_ref("https://github.com/o/r/tree/main/src/app")
        self.assertEqual((r["branch"], r["sub"]), ("main", "src/app"))
        r = rr.repo_ref("https://github.com/a/b.git/")
        self.assertEqual((r["repo"], r["branch"], r["clone_url"]), ("b", None, "https://github.com/a/b"))
        r = rr.repo_ref("https://gitlab.com/group/sub/repo/-/tree/dev")
        self.assertEqual((r["owner"], r["repo"], r["branch"]), ("group-sub", "repo", "dev"))
        r = rr.repo_ref("git@github.com:o/r.git")
        self.assertEqual((r["owner"], r["repo"], r["clone_url"]), ("o", "r", "git@github.com:o/r.git"))
        self.assertIsNone(rr.repo_ref("not a url"))

    def test_repo_key(self):
        self.assertEqual(rr.repo_key("https://GitHub.com/Org/Foo.git"), "https://github.com/Org/Foo")
        self.assertNotEqual(rr.repo_key("https://github.com/org/Foo"), rr.repo_key("https://github.com/org/foo"))
        self.assertEqual(rr.repo_key("git@GitHub.com:o/R.git"), "git@github.com:o/R")

    def test_tokens(self):
        self.assertEqual(rr.token_count("Files analyzed: 5\n\nEstimated tokens: 36.6k"), 36600)
        self.assertEqual(rr.token_count("Estimated tokens: 1.2M"), 1_200_000)
        self.assertEqual(rr.token_count("Estimated tokens: 950"), 950)
        self.assertIsNone(rr.token_count("Files analyzed: 3"))

    def test_patterns(self):
        self.assertEqual(rr.split_patterns(["a,b", " c "]), ["a", "b", "c"])
        self.assertEqual(rr.split_patterns(["./README.md,./docs/*"]), ["/README.md", "/docs/*"])
        with self.assertRaises(rr.clierr.Fail) as cm:
            rr.split_patterns(["My Project/*"])
        self.assertEqual(cm.exception.code, "BAD_ARG")

    def test_excludes(self):
        d = rr.spec(rr.DEFAULT_EXCLUDES)
        self.assertTrue(d.match_file("app/core/src/test/java/A.java"))
        self.assertTrue(d.match_file("frontend/public/locales/de/x.toml"))
        self.assertTrue(d.match_file("src/a.test.ts"))
        self.assertFalse(d.match_file("src/latest/a.ts"))
        self.assertFalse(d.match_file("src/test.ts"))
        self.assertTrue(d.match_file("app/translations/x.json"))
        self.assertTrue(d.match_file("src/snapshots/a.txt"))
        skip = rr.spec(rr.gitingest_ignores() | set(rr.BINARY))
        self.assertTrue(skip.match_file("public/cmaps/Adobe-GB1-0.bcmap"))
        self.assertTrue(skip.match_file("build.gradle"))  # gitingest skips it; digest appends it when named
        self.assertFalse(skip.match_file("src/App.tsx"))

    def test_git_unquote(self):
        self.assertEqual(rr.git_unquote('"src/\\303\\251/main.c"'), "src/" + chr(0xE9) + "/main.c")
        self.assertEqual(rr.git_unquote("plain/path.c"), "plain/path.c")


class Cli(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_bad_arg_one_error_line(self):
        r = run_rr(["map", "not a url"], self.root)
        lines = [l for l in r.stderr.splitlines() if l.strip()]
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertEqual(len(lines), 1, r.stderr)
        self.assertTrue(lines[0].startswith("ERROR rr BAD_ARG: "), lines[0])
        self.assertEqual(len(list((self.root / "logs" / "rr").glob("*"))), 1)

    def test_dot_git_clone_reads_itself(self):
        # gitingest's ingest() stripped ".git" and read the sibling folder "bad" for "bad.git".
        bad = self.root / "src" / "bad.git"
        bad.mkdir(parents=True)
        (self.root / "src" / "bad").mkdir()
        (self.root / "src" / "bad" / "sibling.md").write_text("SIBLING_SECRET\n")
        (bad / "own.md").write_text("OWN_TEXT\n")
        subprocess.run(["git", "-C", str(bad), "init", "-q"], check=True)
        r = run_rr(["digest", "bad.git"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        text = (self.root / "bad.git.txt").read_text()
        self.assertIn("OWN_TEXT", text)
        self.assertNotIn("SIBLING_SECRET", text)

    def make_repo(self, name):
        """A local repo with a docs folder, a tracked tests/a.md, a root build.gradle, a tracked
        .gitingestignore symlink to a file outside the clone, and an untracked git-ignored file."""
        repo = self.root / "src" / name
        (repo / "docs").mkdir(parents=True)
        (repo / "tests").mkdir()
        (repo / "README.md").write_text("# top\n", encoding="utf-8")
        (repo / "docs" / "guide.md").write_text("guide text\n", encoding="utf-8")
        (repo / "docs" / "package-lock.json").write_text('{"lock": "LOCKFILE"}\n', encoding="utf-8")
        (repo / "tests" / "a.md").write_text("test doc A\n", encoding="utf-8")
        (repo / "build.gradle").write_text("plugins { id 'java' }\n", encoding="utf-8")
        (repo / ".gitignore").write_text("ignored.txt\n", encoding="utf-8")
        outside = self.root / "outside.txt"
        outside.write_text("OUTSIDE-SECRET\n", encoding="utf-8")
        (repo / ".gitingestignore").symlink_to(outside)
        for args in (["init", "-q"], ["add", "."], ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"]):
            subprocess.run(["git", "-C", str(repo), *args], check=True)
        (repo / "ignored.txt").write_text("untracked ignored\n", encoding="utf-8")
        return repo

    def test_grep_digest_fixes(self):
        repo = self.make_repo("fx")
        # grep: a path leaving the clone is refused; a single-file operand still shows its path before the line number.
        r = run_rr(["grep", "fx", "top", "../x"], self.root)
        lines = [l for l in r.stderr.splitlines() if l.strip()]
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertEqual(len(lines), 1, r.stderr)
        self.assertTrue(lines[0].startswith("ERROR rr BAD_ARG"), lines[0])
        r = run_rr(["grep", "fx", "plugins", "build.gradle"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(r.stdout.startswith(str(repo / "build.gradle") + ":1:"), r.stdout)

        # A default exclude (tests) stays in when -i names a path under it.
        r = run_rr(["digest", "fx", "-i", "/tests/a.md"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("test doc A", (self.root / "fx.txt").read_text(encoding="utf-8"))
        # A glob names no file outright: build.gradle appears once (gitingest's own pass), not twice.
        r = run_rr(["digest", "fx", "-i", "*.gradle", "--out", "gradle"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual((self.root / "fx-gradle.txt").read_text(encoding="utf-8").count("FILE: build.gradle\n"), 1)
        # A folder named in -i does not pull in the lock file gitingest skips inside it.
        r = run_rr(["digest", "fx", "-i", "docs", "--out", "docs"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        text = (self.root / "fx-docs.txt").read_text(encoding="utf-8")
        self.assertIn("guide text", text)
        self.assertNotIn("LOCKFILE", text)
        # Negation patterns are refused.
        r = run_rr(["digest", "fx", "-i", "!x"], self.root)
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("ERROR rr BAD_ARG", r.stderr)

        # A symlinked .gitingestignore pointing outside is not read; git-ignored files are digested with a warning.
        r = run_rr(["digest", "fx", "--out", "all"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("WARN rr IGNORED_FILES", r.stderr)
        text = (self.root / "fx-all.txt").read_text(encoding="utf-8")
        self.assertNotIn("OUTSIDE-SECRET", text)
        self.assertIn("untracked ignored", text)

    def test_github_in_folder_name_is_local(self):
        # gitingest's ingest() would take this path for a remote URL; the digest must read the folder.
        self.make_repo("owner-github.com")
        r = run_rr(["digest", "owner-github.com", "-i", "*.md", "--out", "gh"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("guide text", (self.root / "owner-github.com-gh.txt").read_text(encoding="utf-8"))

    def test_local_digest(self):
        # A local repo: *.md at any depth, /x anchored at the root, a named *.gradle file appended,
        # a nested test folder excluded by default, a non-ASCII UTF-8 file read intact.
        repo = self.root / "src" / "local-repo"
        (repo / "docs").mkdir(parents=True)
        (repo / "app" / "test").mkdir(parents=True)
        (repo / "README.md").write_text("# top\n", encoding="utf-8")
        (repo / "docs" / "guide.md").write_text("café “quoted” " + "x" * 3000 + "\n", encoding="utf-8")
        (repo / "app" / "test" / "notes.md").write_text("test notes\n", encoding="utf-8")
        (repo / "build.gradle").write_text("plugins { id 'java' }\n", encoding="utf-8")
        (repo / "app" / "build.gradle").write_text("apply plugin: 'app'\n", encoding="utf-8")
        for args in (["init", "-q"], ["add", "."], ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"]):
            subprocess.run(["git", "-C", str(repo), *args], check=True)
        r = run_rr(["digest", "local-repo", "-i", "*.md,/build.gradle"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("files=3", r.stdout)
        self.assertIn("build.gradle", r.stdout)
        text = (self.root / "local-repo.txt").read_text(encoding="utf-8")
        self.assertIn("FILE: README.md", text)
        self.assertIn("café “quoted”", text)
        self.assertIn("plugins { id 'java' }", text)
        self.assertNotIn("test notes", text)
        for anchor in ("/README.md", "./README.md"):
            r = run_rr(["digest", "local-repo", "-i", anchor, "--out", "root"], self.root)
            self.assertIn("files=1", r.stdout, r.stderr)

        # A bare name matches at any depth; built-in ignores beat -i but an -e of the same kind still wins;
        # a path that only reaches a file through ".." matches nothing.
        r = run_rr(["digest", "local-repo", "-i", "build.gradle", "--out", "bare"], self.root)
        self.assertIn("files=2", r.stdout, r.stderr)
        text = (self.root / "local-repo-bare.txt").read_text(encoding="utf-8")
        self.assertIn("FILE: build.gradle\n", text)
        self.assertIn("FILE: app/build.gradle\n", text)
        r = run_rr(["digest", "local-repo", "-i", "/build.gradle", "-e", "*.gradle", "--out", "ex"], self.root)
        self.assertIn("files=0", r.stdout, r.stderr)
        self.assertIn("WARN rr EMPTY", r.stderr)
        r = run_rr(["digest", "local-repo", "-i", "/common/../README.md", "--out", "dots"], self.root)
        self.assertIn("files=0", r.stdout, r.stderr)
        self.assertIn("WARN rr EMPTY", r.stderr)

        # A multi-byte character cut by gitingest's 1024-byte probe is still text.
        (repo / "docs" / "split.md").write_bytes(("x" * 1023 + "é\n").encode("utf-8"))
        r = run_rr(["digest", "local-repo", "-i", "docs/split.md", "--out", "split"], self.root)
        text = (self.root / "local-repo-split.txt").read_text(encoding="utf-8")
        self.assertIn("é", text, r.stderr)
        self.assertNotIn("[Binary file]", text)

        # grep runs where the clone lives and prints paths the caller can open; RR_WIN_PREFIX turns them
        # into \\wsl.localhost paths; --max caps the output with a warning; no match is not an error.
        r = run_rr(["grep", "local-repo", "caf.", "docs"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn(str(repo / "docs" / "guide.md") + ":1:", r.stdout)
        env_win = {"RR_WIN_PREFIX": "\\\\wsl.localhost\\workshop"}
        r = run_rr(["grep", "local-repo", "plugins"], self.root, env_win)
        self.assertIn("\\\\wsl.localhost\\workshop" + str(repo / "build.gradle").replace("/", "\\") + ":1:", r.stdout)
        r = run_rr(["grep", "local-repo", ".", "--max", "1"], self.root)
        self.assertEqual(len(r.stdout.splitlines()), 1, r.stdout)
        self.assertIn("WARN rr MORE", r.stderr)
        r = run_rr(["grep", "local-repo", "zzzz-none"], self.root)
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "no matches"), r.stderr)
        r = run_rr(["checkout", "local-repo", "docs"], self.root)
        self.assertIn("full clone", r.stdout, r.stderr)


@unittest.skipUnless(ONLINE, "set RR_ONLINE=1 for network checks")
class Online(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.root = Path(cls.tmp.name)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_small_repo(self):
        m = run_rr(["map", "https://github.com/coderamp-labs/gitingest"], self.root)
        self.assertEqual(m.returncode, 0, m.stderr)
        self.assertIn("shallow clone", m.stdout)
        self.assertIn("src/gitingest/", m.stdout)
        clone = self.root / "src" / "coderamp-labs-gitingest"
        subprocess.run(["git", "-C", str(clone), "config", "rr.branch", "other"], check=True)
        m2 = run_rr(["map", "https://github.com/coderamp-labs/gitingest"], self.root)
        self.assertEqual(m2.returncode, 1, m2.stderr)
        self.assertIn("holds branch 'other'", m2.stderr)
        subprocess.run(["git", "-C", str(clone), "config", "rr.branch", "main"], check=True)
        m2 = run_rr(["map", "https://github.com/coderamp-labs/gitingest"], self.root)
        self.assertEqual(m2.returncode, 0, m2.stderr)
        self.assertIn("reused", m2.stdout)
        g = run_rr(["digest", "coderamp-labs-gitingest", "-i", "*.md"], self.root)
        self.assertIn("files=5", g.stdout, g.stderr)
        subprocess.run(["git", "-C", str(clone), "checkout", "--detach", "-q"], check=True)
        m2 = run_rr(["map", "https://github.com/coderamp-labs/gitingest"], self.root)
        self.assertNotEqual(m2.returncode, 0, m2.stderr)
        self.assertIn("ERROR rr BAD_INPUT", m2.stderr)

    def test_large_repo_partial(self):
        m = run_rr(["map", "https://github.com/Stirling-Tools/Stirling-PDF"], self.root)
        self.assertEqual(m.returncode, 0, m.stderr)
        self.assertIn("partial clone", m.stdout)
        self.assertIn("app/core/", m.stdout)
        g = run_rr(["digest", "Stirling-Tools-Stirling-PDF", "-i", "MergeController.java,/AGENTS.md,/settings.gradle"], self.root)
        self.assertEqual(g.returncode, 0, g.stderr)
        self.assertIn("files=3", g.stdout, g.stderr)  # MergeController, AGENTS.md (UTF-8), settings.gradle (appended)
        text = (self.root / "Stirling-Tools-Stirling-PDF.txt").read_text(encoding="utf-8")
        self.assertNotIn("Error reading file", text)
        g = run_rr(["digest", "Stirling-Tools-Stirling-PDF"], self.root)
        self.assertEqual(g.returncode, 2, g.stderr)
        self.assertIn("ERROR rr BAD_ARG", g.stderr)
        c = run_rr(["checkout", "Stirling-Tools-Stirling-PDF", "app/core"], self.root)
        self.assertIn("checked out app/core", c.stdout, c.stderr)
        g = run_rr(["grep", "Stirling-Tools-Stirling-PDF", "mergeWithJpdfium", "app/core"], self.root)
        self.assertIn("MergeController.java:", g.stdout, g.stderr)


if __name__ == "__main__":
    unittest.main()

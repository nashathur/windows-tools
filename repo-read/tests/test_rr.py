"""rr.py checks: URL parsing, pattern handling, the structure map, one BAD_ARG error (one ERROR line plus its
log), and grep/checkout on a local repo. RR_ONLINE=1 also clones a small GitHub repo and a partial clone of a
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

    def test_patterns(self):
        self.assertEqual(rr.split_patterns(["a,b", " c "]), ["a", "b", "c"])
        self.assertEqual(rr.split_patterns(["./README.md,./docs/*"]), ["/README.md", "/docs/*"])
        self.assertEqual(rr.split_patterns(["My Project/*"]), ["My Project/*"])

    def test_git_unquote(self):
        self.assertEqual(rr.git_unquote('"src/\\303\\251/main.c"'), "src/" + chr(0xE9) + "/main.c")
        self.assertEqual(rr.git_unquote("plain/path.c"), "plain/path.c")

    def test_render_map(self):
        paths = ["README.md", "Dockerfile", "pom.xml",
                 *[f"app/core/src/A{i}.java" for i in range(5)], "app/core/src/b.xml", "app/core/pom.xml",
                 "app/core/src/Application.java", "app/web/package.json", "app/web/src/a.ts",
                 "src/tests/README.md", "node_modules/x/package.json", "docs/a/b/c/README.md",
                 ".github/workflows/release.yml", ".github/workflows/ci.yml", ".gitlab-ci.yml"]
        lines = rr.render_map(paths, 2)
        self.assertIn("     4 files  (root files)  Dockerfile 1, md 1", lines)
        self.assertIn("     8 files  app/core/  java 6, xml 2", lines)
        self.assertIn("     2 files  app/web/  json 1, ts 1", lines)
        self.assertIn("     1 files  docs/a/  md 1", lines)
        self.assertIn("     2 files  .github/workflows/  yml 2", lines)
        i = lines.index("key files:")
        keys = [l.strip() for l in lines[i + 1:-1]]
        self.assertEqual(keys, ["Dockerfile", "README.md", "pom.xml", "app/core/pom.xml", "app/web/package.json",
                                "app/core/src/Application.java"])  # shallow first; tests/, node_modules/ and depth 5 out
        self.assertEqual(lines[-1], "workflows: 2 in .github/workflows (ci.yml, release.yml); .gitlab-ci.yml")
        self.assertEqual(rr.render_map(["a/b/c.py"], 1), ["     1 files  a/  py 1", "", "key files:"])

    def test_render_map_row_cap(self):
        lines = rr.render_map([f"d{i:02d}/f.txt" for i in range(70)], 1)
        self.assertEqual(sum("files  d" in l for l in lines), rr.MAX_ROWS)
        self.assertIn("... +10 more folders (rr clone again with --depth 1, or grep)", lines)
        self.assertEqual(rr.render_map(["f.txt"] * 3, 2)[0], "     3 files  (root files)  txt 3")


class Cli(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_bad_arg_one_error_line(self):
        r = run_rr(["clone", "not a url"], self.root)
        lines = [l for l in r.stderr.splitlines() if l.strip()]
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertEqual(len(lines), 1, r.stderr)
        self.assertTrue(lines[0].startswith("ERROR rr BAD_ARG: "), lines[0])
        self.assertEqual(len(list((self.root / "logs" / "rr").glob("*"))), 1)

    def make_repo(self, name):
        repo = self.root / "src" / name
        (repo / "docs").mkdir(parents=True)
        (repo / "README.md").write_text("# top\n", encoding="utf-8")
        (repo / "docs" / "guide.md").write_text("café " + "x" * 300 + "\n", encoding="utf-8")
        (repo / "build.gradle").write_text("plugins { id 'java' }\n", encoding="utf-8")
        for args in (["init", "-q"], ["add", "."], ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"]):
            subprocess.run(["git", "-C", str(repo), *args], check=True)
        return repo

    def test_grep_and_checkout_local(self):
        repo = self.make_repo("fx")
        # A path leaving the clone is refused with one error line.
        r = run_rr(["grep", "fx", "top", "../x"], self.root)
        lines = [l for l in r.stderr.splitlines() if l.strip()]
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertEqual(len(lines), 1, r.stderr)
        self.assertTrue(lines[0].startswith("ERROR rr BAD_ARG"), lines[0])
        # A single-file operand still shows its path before the line number.
        r = run_rr(["grep", "fx", "plugins", "build.gradle"], self.root)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(r.stdout.startswith(str(repo / "build.gradle") + ":1:"), r.stdout)
        r = run_rr(["grep", "fx", "caf.", "docs"], self.root)
        self.assertIn(str(repo / "docs" / "guide.md") + ":1:", r.stdout)
        # RR_WIN_PREFIX turns paths into \\wsl.localhost paths.
        env_win = {"RR_WIN_PREFIX": "\\\\wsl.localhost\\workshop"}
        r = run_rr(["grep", "fx", "plugins"], self.root, env_win)
        self.assertIn("\\\\wsl.localhost\\workshop" + str(repo / "build.gradle").replace("/", "\\") + ":1:", r.stdout)
        # --max caps the output with a warning; no match is not an error.
        r = run_rr(["grep", "fx", ".", "--max", "1"], self.root)
        self.assertEqual(len(r.stdout.splitlines()), 1, r.stdout)
        self.assertIn("WARN rr MORE", r.stderr)
        r = run_rr(["grep", "fx", "zzzz-none"], self.root)
        self.assertEqual((r.returncode, r.stdout.strip()), (0, "no matches"), r.stderr)
        r = run_rr(["checkout", "fx", "docs"], self.root)
        self.assertIn("full clone", r.stdout, r.stderr)
        r = run_rr(["grep", "nope", "x"], self.root)
        self.assertIn("rr.py clone <url>", r.stderr)


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
        url = "https://github.com/coderamp-labs/gitingest"
        m = run_rr(["clone", url], self.root)
        self.assertEqual(m.returncode, 0, m.stderr)
        self.assertIn("shallow clone", m.stdout)
        self.assertIn("src/gitingest/", m.stdout)
        self.assertIn("key files:", m.stdout)
        clone = self.root / "src" / "coderamp-labs-gitingest"
        subprocess.run(["git", "-C", str(clone), "config", "rr.branch", "other"], check=True)
        m2 = run_rr(["clone", url], self.root)
        self.assertEqual(m2.returncode, 1, m2.stderr)
        self.assertIn("holds branch 'other'", m2.stderr)
        subprocess.run(["git", "-C", str(clone), "config", "rr.branch", "main"], check=True)
        m2 = run_rr(["clone", url], self.root)
        self.assertEqual(m2.returncode, 0, m2.stderr)
        self.assertIn("reused", m2.stdout)
        subprocess.run(["git", "-C", str(clone), "checkout", "--detach", "-q"], check=True)
        m2 = run_rr(["clone", url], self.root)
        self.assertNotEqual(m2.returncode, 0, m2.stderr)
        self.assertIn("ERROR rr BAD_INPUT", m2.stderr)

    def test_large_repo_partial(self):
        m = run_rr(["clone", "https://github.com/Stirling-Tools/Stirling-PDF"], self.root)
        self.assertEqual(m.returncode, 0, m.stderr)
        self.assertIn("partial clone", m.stdout)
        self.assertIn("app/core/", m.stdout)
        c = run_rr(["checkout", "Stirling-Tools-Stirling-PDF", "app/core"], self.root)
        self.assertIn("checked out app/core", c.stdout, c.stderr)
        g = run_rr(["grep", "Stirling-Tools-Stirling-PDF", "mergeWithJpdfium", "app/core"], self.root)
        self.assertIn("MergeController.java:", g.stdout, g.stderr)
        self.assertIn("MergeController", g.stdout)


if __name__ == "__main__":
    unittest.main()

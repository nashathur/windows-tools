import argparse
import importlib.util
import inspect
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))  # fixtures import `tests.fixtures` when run as a script


def build_all(folder, bench=False):
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    result = {}
    for path in sorted((Path(__file__).parent / "fixtures").glob("p*.py")):
        spec = importlib.util.spec_from_file_location(f"fixture_{path.stem}", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        target = folder / path.stem
        if "bench" in inspect.signature(module.build_fixtures).parameters:
            built = module.build_fixtures(target, bench=bench)
        else:
            built = module.build_fixtures(target)
        result[path.stem] = built
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(allow_abbrev=False)
    parser.add_argument("folder", nargs="?", type=Path, default=Path(__file__).parent / "fixtures" / "out")
    parser.add_argument("--bench", action="store_true")
    args = parser.parse_args(argv)
    build_all(args.folder, bench=args.bench)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

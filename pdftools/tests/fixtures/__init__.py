from pathlib import Path

# Arial on Windows, its metric twin Liberation Sans on Linux; both cover Greek.
_SANS_CANDIDATES = (Path(r"C:\Windows\Fonts\arial.ttf"),
                    Path("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf"),
                    Path("/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf"))


def sans_font():
    for path in _SANS_CANDIDATES:
        if path.is_file():
            return str(path)
    raise FileNotFoundError("no Arial or Liberation Sans font found; install fonts-liberation")

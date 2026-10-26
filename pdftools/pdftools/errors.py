import clierr

library_warnings = []


class BadArg(clierr.Fail):
    def __init__(self, message, problems=()):
        problems = tuple(problems) or (message,)
        super().__init__("BAD_ARG", message, "correct the command or job", detail="\n".join(problems))
        self.problems = tuple(problems)


class PdfError(clierr.Fail):
    def __init__(self, code, what, fix, detail=None):
        super().__init__(code, what, fix, detail=detail)


def bad_arg(message, problems=()):
    return PdfError("BAD_ARG", message, "correct the command or job", detail="\n".join(problems))

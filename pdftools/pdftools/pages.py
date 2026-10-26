from .errors import PdfError


def parse_page_ranges(spec, page_count):
    if page_count < 1:
        raise PdfError("BAD_PAGE", "document has no pages", "use a PDF with pages")
    result = []
    try:
        for part in str(spec).split(","):
            part = part.strip().lower()
            if part == "odd":
                values = range(1, page_count + 1, 2)
            elif part == "even":
                values = range(2, page_count + 1, 2)
            elif "-" in part:
                a, b = part.split("-", 1)
                start, end = int(a) if a else 1, int(b) if b else page_count
                if (a and (start < 1 or start > page_count)) or (b and (end < 1 or end > page_count)):
                    raise ValueError
                if not a or not b or start <= end:
                    values = range(start, end + 1)
                else:
                    values = range(start, end - 1, -1)
            else:
                values = [int(part)]
            for number in values:
                if number < 1 or number > page_count:
                    raise ValueError
                result.append(number - 1)
    except (ValueError, TypeError):
        raise PdfError("BAD_PAGE", f"invalid page range: {spec}", f"use page numbers from 1 to {page_count}")
    if not result:
        raise PdfError("BAD_PAGE", f"page selection is empty: {spec}", "choose at least one page from 1 to {page_count}")
    return result

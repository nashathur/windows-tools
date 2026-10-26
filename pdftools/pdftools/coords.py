import pymupdf

MM_PER_POINT = 25.4 / 72


def _user_unit(page):
    value_type, value = page.parent.xref_get_key(page.xref, "UserUnit")
    try:
        return float(value) if value_type == "int" else float(value) if value_type == "real" else 1.0
    except ValueError:
        return 1.0


def pymupdf_to_display(page, rect):
    r = pymupdf.Rect(rect) * page.rotation_matrix
    factor = MM_PER_POINT * _user_unit(page)
    return pymupdf.Rect(r.x0 * factor, r.y0 * factor, r.x1 * factor, r.y1 * factor)


def display_to_pymupdf(page, rect):
    r = pymupdf.Rect(rect)
    factor = MM_PER_POINT * _user_unit(page)
    r = pymupdf.Rect(r.x0 / factor, r.y0 / factor, r.x1 / factor, r.y1 / factor)
    return r * page.derotation_matrix


def to_user_space(page, rect):
    return pymupdf.Rect(rect) * ~page.transformation_matrix


def from_user_space(page, rect):
    return pymupdf.Rect(rect) * page.transformation_matrix

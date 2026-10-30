"""Conversions between PDF, PyMuPDF, and displayed page coordinates.

PyMuPDF text geometry is scaled by /UserUnit, while its page matrices are not.
PDF user coordinates are unscaled and use a bottom-left origin; text coordinates
use a top-left origin. Annotation objects' raw PDF ``/Rect`` is converted through
PDF user space because ``annot.rect`` uses MuPDF's rotated annotation geometry.
"""
import re

import pymupdf

MM_PER_POINT = 25.4 / 72


def _user_unit(page):
    value_type, value = page.parent.xref_get_key(page.xref, "UserUnit")
    try:
        value = float(value) if value_type in {"int", "real", "float"} else 1.0
        return value if value > 0 else 1.0
    except (TypeError, ValueError):
        return 1.0


def _pdf_box(page):
    """Raw PDF CropBox (or MediaBox) coordinates, before /UserUnit scaling."""
    media = pymupdf.Rect(page.mediabox)
    crop = pymupdf.Rect(page.cropbox)
    # PyMuPDF resolves inherited and indirect boxes. Its cropbox is top-left
    # oriented within the MediaBox, while PDF user space is bottom-left oriented.
    return pymupdf.Rect(crop.x0, media.y1 - crop.y1,
                        crop.x1, media.y1 - crop.y0)


def to_user_space(page, rect):
    """Convert PyMuPDF text-space geometry to unscaled PDF user space."""
    r = pymupdf.Rect(rect)
    box = _pdf_box(page)
    unit = _user_unit(page)
    return pymupdf.Rect(r.x0 / unit + box.x0, box.y1 - r.y1 / unit,
                        r.x1 / unit + box.x0, box.y1 - r.y0 / unit)


def from_user_space(page, rect):
    """Convert unscaled PDF user geometry to PyMuPDF text space."""
    r = pymupdf.Rect(rect)
    box = _pdf_box(page)
    unit = _user_unit(page)
    return pymupdf.Rect((r.x0 - box.x0) * unit,
                        (box.y1 - r.y1) * unit,
                        (r.x1 - box.x0) * unit,
                        (box.y1 - r.y0) * unit)


def to_displayed_points(page, rect):
    """Convert PyMuPDF text-space geometry to displayed point coordinates."""
    unit = _user_unit(page)
    return (pymupdf.Rect(rect) * (1 / unit)) * page.rotation_matrix * unit


def unrotated_render_matrix(page, matrix):
    """Return a pixmap matrix that renders the page without its /Rotate value."""
    return page.derotation_matrix * matrix


def displayed_matrix(page):
    """Affine map from PyMuPDF text-space points to displayed points."""
    rotation = page.rotation_matrix
    unit = _user_unit(page)
    return pymupdf.Matrix(rotation.a, rotation.b, rotation.c, rotation.d,
                          rotation.e * unit, rotation.f * unit)


def displayed_points_to_mm(rect):
    """Convert already-displayed point geometry to displayed millimetres."""
    return pymupdf.Rect(rect) * MM_PER_POINT


def mm_to_points(value):
    """Convert millimetres to displayed points."""
    return float(value) / MM_PER_POINT


def displayed_mm_to_points(rect):
    """Convert displayed millimetre rectangle geometry to displayed points."""
    return pymupdf.Rect(rect) / MM_PER_POINT


def display_to_pymupdf(page, rect):
    """Convert displayed millimetres to PyMuPDF text-space geometry."""
    unit = _user_unit(page)
    r = pymupdf.Rect(rect) / (MM_PER_POINT * unit)
    return (r * page.derotation_matrix) * unit


def pymupdf_to_display(page, rect):
    """Convert PyMuPDF text-space geometry to displayed millimetres."""
    r = to_displayed_points(page, rect)
    return displayed_points_to_mm(r)


def annotation_to_pymupdf(page, rect):
    """Convert an annotation or widget rectangle to PyMuPDF text space.

    Annotation objects expose ``rect`` in a MuPDF-specific rotated space. Their
    raw PDF ``/Rect`` is the stable source of truth, in PDF user coordinates.
    A plain rectangle is treated as already being in text space.
    """
    if hasattr(rect,"xref"):
        kind,value=page.parent.xref_get_key(rect.xref,"Rect")
        if kind=="array":
            numbers=re.findall(r"[-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][-+]?\d+)?",value)
            if len(numbers)>=4:
                return from_user_space(page,pymupdf.Rect(*(float(number) for number in numbers[:4])))
        rect=rect.rect
    return pymupdf.Rect(rect)


def pymupdf_to_annotation(page, rect):
    """Convert text-space geometry to the coordinates accepted by annotation APIs."""
    return pymupdf.Rect(rect)


def cropbox_to_pymupdf(page, rect):
    """Convert absolute, unscaled PyMuPDF crop-box coordinates to text space."""
    box = page.cropbox
    unit = _user_unit(page)
    r = pymupdf.Rect(rect)
    return pymupdf.Rect((r.x0 - box.x0) * unit, (r.y0 - box.y0) * unit,
                        (r.x1 - box.x0) * unit, (r.y1 - box.y0) * unit)


def pymupdf_to_cropbox(page, rect):
    """Convert PyMuPDF text-space geometry to absolute crop-box coordinates."""
    box = page.cropbox
    unit = _user_unit(page)
    r = pymupdf.Rect(rect) / unit
    return pymupdf.Rect(r.x0 + box.x0, r.y0 + box.y0,
                        r.x1 + box.x0, r.y1 + box.y0)


def cropbox_to_pdf_space(page, rect):
    """Convert absolute PyMuPDF crop coordinates to PDF user-space coordinates."""
    r = pymupdf.Rect(rect)
    media = pymupdf.Rect(page.mediabox)
    return pymupdf.Rect(r.x0, media.y1 - r.y1, r.x1, media.y1 - r.y0)


def unrotated_points_to_cropbox(page, rect):
    """Convert points measured in an unrotated render to absolute crop-box coordinates."""
    box = page.cropbox
    r = pymupdf.Rect(rect) / _user_unit(page)
    return pymupdf.Rect(r.x0 + box.x0, r.y0 + box.y0,
                        r.x1 + box.x0, r.y1 + box.y0)


def physical_size(page, rect):
    """Return physical dimensions in points for a rectangle in text space."""
    user_rect = to_user_space(page, rect)
    unit = _user_unit(page)
    return pymupdf.Point(user_rect.width * unit, user_rect.height * unit)


def cut_panel_geometry(page, displayed_box):
    """Return source clip, unrotated destination bounds, and placement matrix."""
    box = pymupdf.Rect(displayed_box)
    clip = display_to_pymupdf(page, displayed_points_to_mm(box))
    panel_box = pymupdf_to_cropbox(page, clip)
    dest_rect = pymupdf.Rect(0, 0, clip.width, clip.height)
    matrix = (1, 0, 0, 1, -clip.x0, -clip.y0)
    return clip, panel_box, dest_rect, matrix

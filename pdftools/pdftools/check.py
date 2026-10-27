from dataclasses import dataclass, field

import cv2
import numpy as np
import pymupdf

from . import coords
from .registry import BUILTIN_POLICIES

TILE_MM = 10.0
RENDER_DPI = 100
TILE_CHANGED_FRACTION = 0.01
CHANNEL_DELTA = 8
SSIM_MED = 0.90
SSIM_HIGH = 0.85
SSIM_MAX = 0.78
GRAY_CHROMA_MAX = 2
GRAY_LUMINANCE_MAX_DELTA = 24
TEXT_SIMILARITY_MIN = 0.98


@dataclass(frozen=True)
class Placement:
    source_doc: str | None
    source_page: int | str
    transform: tuple = (1, 0, 0, 1, 0, 0)
    dest_rect: tuple | None = None
    rotation: int | None = None


@dataclass
class ExpectedChanges:
    """Allowed regions are rectangles in displayed millimetres on output pages."""
    regions: list = field(default_factory=list)
    text_added: list = field(default_factory=list)
    text_removed: list = field(default_factory=list)


@dataclass
class Features:
    """must_survive gives exact output counts; dropped gives source counts removed."""
    must_survive: dict = field(default_factory=dict)
    dropped: dict = field(default_factory=dict)


@dataclass
class OperationResult:
    pages: list = field(default_factory=list)
    expected: ExpectedChanges = field(default_factory=ExpectedChanges)
    features: Features = field(default_factory=Features)
    ssim_floor: float | None = None
    annotation_delta: int | None = None
    cropboxes: list = field(default_factory=list)


@dataclass
class Finding:
    tier: str
    code: str
    message: str
    page: int | None = None
    details: dict = field(default_factory=dict)


def normalize_words(words):
    text = " ".join(str(w) for w in words).replace("\u00ad", "")
    text = text.replace("ﬁ", "fi").replace("ﬂ", "fl").replace("ﬀ", "ff")
    text = text.replace("\u2010", "-").replace("\u2011", "-")
    text = text.replace("- ", "")
    return tuple(text.split())


def page_words(page):
    return normalize_words(w[4] for w in page.get_text("words"))


def render_rgb(page, dpi=RENDER_DPI, clip=None, *, annots=False, unrotated=False):
    matrix = pymupdf.Matrix(dpi / 72, dpi / 72)
    if unrotated:
        matrix = page.derotation_matrix * matrix
    pix = page.get_pixmap(matrix=matrix, clip=clip, alpha=False, annots=annots)
    return np.frombuffer(pix.samples, dtype=np.uint8).reshape(pix.height, pix.width, pix.n).copy()


def tile_differences(source, output, page_no=1, tile_px=None, page=None):
    """Return changed tile boxes in displayed millimetres and measured deltas."""
    h, w = min(source.shape[0], output.shape[0]), min(source.shape[1], output.shape[1])
    if h == 0 or w == 0:
        return [{"box_mm": (0, 0, 0, 0), "fraction": 1.0, "max_delta": 255}]
    a, b = source[:h, :w], output[:h, :w]
    tile_px = tile_px or max(1, round(TILE_MM * RENDER_DPI / 25.4))
    changed = []
    for y in range(0, h, tile_px):
        for x in range(0, w, tile_px):
            aa, bb = a[y:y + tile_px, x:x + tile_px], b[y:y + tile_px, x:x + tile_px]
            delta = np.abs(aa.astype(np.int16) - bb.astype(np.int16))
            mask = np.any(delta > CHANNEL_DELTA, axis=2)
            fraction = float(mask.mean())
            if fraction > TILE_CHANGED_FRACTION:
                origin = page.cropbox if page is not None else pymupdf.Rect(0, 0, 0, 0)
                box_mm = pymupdf.Rect(origin.x0 + x * 72 / RENDER_DPI, origin.y0 + y * 72 / RENDER_DPI,
                                      origin.x0 + min(x + tile_px, w) * 72 / RENDER_DPI,
                                      origin.y0 + min(y + tile_px, h) * 72 / RENDER_DPI)
                if page is not None:
                    box_mm = coords.pymupdf_to_display(page, box_mm)
                changed.append({"box_mm": tuple(box_mm),
                                "fraction": fraction, "max_delta": int(delta.max())})
    return changed


def ssim_gray(a, b):
    a = cv2.cvtColor(a, cv2.COLOR_RGB2GRAY).astype(np.float32) / 255
    b = cv2.cvtColor(b, cv2.COLOR_RGB2GRAY).astype(np.float32) / 255
    if a.shape != b.shape:
        b = cv2.resize(b, (a.shape[1], a.shape[0]), interpolation=cv2.INTER_AREA)
    c1, c2 = 0.01 ** 2, 0.03 ** 2
    mu_a, mu_b = cv2.GaussianBlur(a, (11, 11), 1.5), cv2.GaussianBlur(b, (11, 11), 1.5)
    sigma_a = cv2.GaussianBlur(a * a, (11, 11), 1.5) - mu_a * mu_a
    sigma_b = cv2.GaussianBlur(b * b, (11, 11), 1.5) - mu_b * mu_b
    sigma_ab = cv2.GaussianBlur(a * b, (11, 11), 1.5) - mu_a * mu_b
    value = ((2 * mu_a * mu_b + c1) * (2 * sigma_ab + c2)) / ((mu_a ** 2 + mu_b ** 2 + c1) * (sigma_a + sigma_b + c2))
    return float(value.mean())


def page_stats(page, renderer=render_rgb):
    words = page.get_text("words")
    pix = renderer(page, 50)
    gray = cv2.cvtColor(pix, cv2.COLOR_RGB2GRAY)
    images = page.get_image_info(xrefs=True)
    unit = coords._user_unit(page)
    image_dpi = []
    for info in images:
        rect = pymupdf.Rect(info["bbox"])
        dpi_x = info["width"] * 72 / max(rect.width * unit, 0.01)
        dpi_y = info["height"] * 72 / max(rect.height * unit, 0.01)
        image_dpi.append(round(min(dpi_x, dpi_y), 1))
    spans = [span for block in page.get_text("dict").get("blocks", []) for line in block.get("lines", [])
             for span in line.get("spans", [])]
    outside = [span for span in spans if not page.rect.contains(pymupdf.Rect(span["bbox"]))]
    return {"blank": not words and int(np.count_nonzero(gray < 245)) == 0,
            "mostly_black": float(np.mean(gray < 16)) > 0.95,
            "ink_coverage": float(np.mean(gray < 245)),
            "smallest_font_pt": min((s["size"] for s in spans), default=None),
            "replacement_char": "\ufffd" in page.get_text("text"),
            "notdef": any(".notdef" in s.get("font", "") for s in spans),
            "spans_outside_crop": len(outside),
            "image_count": len(images),
            "image_dpi": image_dpi,
            "max_chroma": int(np.max(np.max(pix, axis=2).astype(np.int16) - np.min(pix, axis=2).astype(np.int16)))}


def _feature_counts(doc):
    return {"bookmarks": len(doc.get_toc()), "labels": len(doc.get_page_labels()),
            "links": sum(len(page.get_links()) for page in doc),
            "widgets": sum(sum(1 for _ in (page.widgets() or [])) for page in doc),
            "annotations": sum(sum(1 for _ in (page.annots() or [])) for page in doc)}


def _tiers(source_page, output_page, page_no, renderer=render_rgb):
    before, after = page_stats(source_page, renderer), page_stats(output_page, renderer)
    pairs = (("blank", "BLANK_PAGE"), ("mostly_black", "MOSTLY_BLACK"),
             ("replacement_char", "MISSING_GLYPH"), ("notdef", "MISSING_GLYPH"),
             ("spans_outside_crop", "TEXT_OUTSIDE_CROP"))
    results = []
    for key, code in pairs:
        old, new = bool(before[key]), bool(after[key])
        if new and not old:
            results.append(Finding("new_warning", code, f"new {key.replace('_', ' ')} finding", page_no))
        elif new and old:
            results.append(Finding("pre_existing", code, f"pre-existing {key.replace('_', ' ')} finding", page_no))
    for dpi in after["image_dpi"] if not page_words(output_page) and after["image_count"] == 1 else ():
        if dpi and dpi < 150:
            tier = "pre_existing" if any(v and v < 150 for v in before["image_dpi"]) else "new_warning"
            results.append(Finding(tier, "LOW_DPI", "image effective dpi is below 150", page_no, {"dpi": dpi}))
    return results


def _remove_words(words, expected):
    from collections import Counter
    remove = Counter(normalize_words(expected))
    result = []
    for word in words:
        if remove[word]:
            remove[word] -= 1
        else:
            result.append(word)
    return tuple(result)


def _ordered_subset(needles, haystack):
    cursor = iter(haystack)
    return all(any(candidate == word for candidate in cursor) for word in needles)


def _inside_expected(change, operation, page_no):
    if not operation:
        return False
    x0, y0, x1, y1 = change["box_mm"]
    for region in operation.expected.regions:
        if isinstance(region, dict):
            if region.get("page") not in (None, page_no):
                continue
            rect = region.get("rect", region.get("region"))
        else:
            rect = region
        if rect is None:
            continue
        rx0, ry0, rx1, ry1 = map(float, rect)
        if rx0 < x1 and x0 < rx1 and ry0 < y1 and y0 < ry1:
            return True
    return False


def _inside_image_region(change, placements, source, source_docs, output_page):
    x0,y0,x1,y1=change["box_mm"]
    tile=pymupdf.Rect(x0,y0,x1,y1)
    for placement in placements:
        page=_placement_source(placement,source,source_docs)
        if page is None:
            continue
        a,b,c,d,e,f=placement.transform
        for info in page.get_image_info(xrefs=True):
            r=pymupdf.Rect(info["bbox"])
            pts=[pymupdf.Point(a*x+c*y+e,b*x+d*y+f) for x,y in ((r.x0,r.y0),(r.x1,r.y0),(r.x0,r.y1),(r.x1,r.y1))]
            mapped=coords.pymupdf_to_display(output_page,pymupdf.Rect(min(p.x for p in pts),min(p.y for p in pts),max(p.x for p in pts),max(p.y for p in pts)))
            if tile.intersects(mapped):
                return True
    return False


def _placement_object(value, default_page):
    if isinstance(value, Placement):
        return value
    return Placement(value.get("source_doc"), value.get("source_page", default_page),
                     tuple(value.get("transform", (1, 0, 0, 1, 0, 0))), value.get("dest_rect"),
                     value.get("rotation"))


def _placement_source(placement, default_doc, source_docs):
    doc = default_doc
    if placement.source_doc and source_docs:
        from pathlib import Path
        doc = source_docs.get(str(placement.source_doc), source_docs.get(str(Path(placement.source_doc).resolve()), doc))
    if doc is None or not isinstance(placement.source_page, int) or not 0 <= placement.source_page < doc.page_count:
        return None
    return doc[placement.source_page]


def _placement_words(placement, page, policy, index, operation):
    words = page.get_text("words")
    if policy == "crop" and operation and index < len(operation.cropboxes):
        crop = pymupdf.Rect(operation.cropboxes[index])
        return normalize_words(w[4] for w in words if crop.contains(pymupdf.Point((w[0]+w[2])/2, (w[1]+w[3])/2)))
    if placement.dest_rect:
        region = _source_region(placement)
        if region is not None:
            words = [w for w in words if region.contains(pymupdf.Point((w[0]+w[2])/2, (w[1]+w[3])/2))]
    return normalize_words(w[4] for w in words)


def _source_region(placement):
    if not placement.dest_rect:
        return None
    a,b,c,d,e,f = placement.transform
    det = a*d-b*c
    if abs(det) < 1e-9:
        return None
    dest = pymupdf.Rect(placement.dest_rect)
    points = [pymupdf.Point((d*(x-e)-c*(y-f))/det, (-b*(x-e)+a*(y-f))/det)
              for x,y in ((dest.x0,dest.y0),(dest.x1,dest.y0),
                          (dest.x0,dest.y1),(dest.x1,dest.y1))]
    return pymupdf.Rect(min(p.x for p in points), min(p.y for p in points),
                        max(p.x for p in points), max(p.y for p in points))


def _placement_canvas(placements, default_doc, source_docs, output_page, output_rgb, renderer=render_rgb):
    height, width = output_rgb.shape[:2]
    canvas = np.full_like(output_rgb, 255)
    output_rect = output_page.cropbox
    scale = RENDER_DPI / 72
    for placement in placements:
        source_page = _placement_source(placement, default_doc, source_docs)
        if source_page is None:
            continue
        source_rgb = renderer(source_page, unrotated=True)
        source_rect = source_page.cropbox
        a, b, c, d, e, f = placement.transform
        affine = np.array([[a, c, (a * source_rect.x0 + c * source_rect.y0 + e - output_rect.x0) * scale],
                           [b, d, (b * source_rect.x0 + d * source_rect.y0 + f - output_rect.y0) * scale]], dtype=np.float64)
        warped = cv2.warpAffine(source_rgb, affine, (width, height), flags=cv2.INTER_LINEAR,
                                borderMode=cv2.BORDER_CONSTANT, borderValue=(255, 255, 255))
        source_mask = (np.min(source_rgb, axis=2) < 250).astype(np.uint8) * 255
        region = _source_region(placement)
        if region is not None:
            yy,xx=np.ogrid[:source_rgb.shape[0],:source_rgb.shape[1]]
            inside=(((xx/scale+source_rect.x0)>=region.x0)&((xx/scale+source_rect.x0)<=region.x1)&
                    ((yy/scale+source_rect.y0)>=region.y0)&((yy/scale+source_rect.y0)<=region.y1))
            source_mask[~inside]=0
        mask = cv2.warpAffine(source_mask, affine, (width, height), flags=cv2.INTER_NEAREST,
                              borderMode=cv2.BORDER_CONSTANT, borderValue=0) > 0
        canvas[mask] = warped[mask]
    return canvas


def compare(source, output, policy="placements", operation=None, *, source_docs=None):
    if callable(policy):
        return policy(source, output, operation)
    if policy not in BUILTIN_POLICIES:
        raise ValueError(f"unknown invariant policy: {policy}")
    findings = []
    render_cache = {}

    def rendered(page, dpi=RENDER_DPI, clip=None, *, annots=False, unrotated=False):
        key=(id(page.parent), page.number, page.xref, dpi, annots, unrotated,
             tuple(clip) if clip is not None else None)
        if key not in render_cache:
            render_cache[key]=render_rgb(page, dpi, clip, annots=annots, unrotated=unrotated)
        return render_cache[key]
    if policy == "report":
        return findings
    expected_pages = len(operation.pages) if operation and operation.pages else len(source)
    if len(output) != expected_pages:
        findings.append(Finding("invariant", "PAGE_COUNT", "page count differs from the page map",
                                details={"expected": expected_pages, "output": len(output)}))
    for i in range(len(output)):
        b = output[i]
        raw_placements = operation.pages[i] if operation and i < len(operation.pages) else [Placement(None, i)]
        if not isinstance(raw_placements, (list, tuple)):
            raw_placements = [raw_placements]
        placements = [_placement_object(value, i) for value in raw_placements]
        page_sources = [_placement_source(placement, source, source_docs) for placement in placements]
        page_sources = [page for page in page_sources if page is not None]
        if not page_sources and placements and all(p.source_page == "generated" for p in placements):
            continue
        if policy in {"placements", "rotate", "stamp", "annot_add", "annot_remove", "flatten", "flatten_raster", "crop", "rmblank", "structure", "gray", "compress_lossless", "compress_lossy", "ocr"}:
            expected_rotations = [p.rotation for p in placements if p.rotation is not None]
            if expected_rotations and b.rotation != expected_rotations[-1]:
                findings.append(Finding("invariant", "ROTATION_CHANGED", "output page rotation differs from the operation",
                                        i + 1, {"expected": expected_rotations[-1], "actual": b.rotation}))
        if policy == "crop" and operation and i < len(operation.cropboxes):
            expected_box = pymupdf.Rect(operation.cropboxes[i])
            if any(abs(actual - expected) > 0.01 for actual, expected in zip(b.cropbox, expected_box)):
                findings.append(Finding("invariant", "CROPBOX_CHANGED", "output crop box differs from the operation",
                                        i + 1, {"expected": tuple(expected_box), "actual": tuple(b.cropbox)}))
        if not page_sources and placements and all(p.source_page == "blank" for p in placements):
            stats = page_stats(b)
            if not stats["blank"]:
                findings.append(Finding("invariant", "BLANK_PLACEMENT_NOT_BLANK", "blank output page contains words or ink", i + 1))
            continue
        wa = tuple(word for placement, page in zip(placements, page_sources)
                   for word in _placement_words(placement, page, policy, i, operation))
        output_word_rows = b.get_text("words")
        output_regions = [pymupdf.Rect(p.dest_rect) for p in placements if p.dest_rect]
        if output_regions and policy not in {"stamp", "annot_add", "annot_remove", "flatten", "ocr"}:
            output_word_rows = [w for w in output_word_rows
                                if any(region.contains(pymupdf.Point((w[0]+w[2])/2+b.cropbox.x0,
                                                                    (w[1]+w[3])/2+b.cropbox.y0))
                                       for region in output_regions)]
        wb = normalize_words(w[4] for w in output_word_rows)
        expected = operation.expected if operation else ExpectedChanges()
        source_cmp = wa if policy == "stamp" else _remove_words(wa, expected.text_removed)
        output_cmp = wb if policy == "stamp" else _remove_words(wb, expected.text_added)
        if expected.text_added:
            from collections import Counter
            required = Counter(wa) + Counter(normalize_words(expected.text_added))
            available = Counter(wb)
            missing = {word: count - available[word] for word, count in required.items()
                       if available[word] < count}
            if missing:
                findings.append(Finding("invariant", "TEXT_ADDITION_MISSING", "planned added text is missing",
                                        i + 1, {"missing": missing}))
        if policy in {"placements", "rotate", "stamp", "annot_add", "annot_remove", "flatten", "crop", "rmblank", "structure", "gray", "compress_lossless", "compress_lossy", "ocr"}:
            text_matches = (source_cmp == output_cmp)
            if policy in {"stamp", "flatten", "ocr", "annot_add", "annot_remove"}:
                text_matches = _ordered_subset(source_cmp, output_cmp)
            if policy == "flatten_raster":
                text_matches = True
            if not text_matches:
                findings.append(Finding("invariant", "TEXT_CHANGED", "page text changed", i + 1,
                                       {"source_words": len(wa), "output_words": len(wb)}))
            if policy == "ocr" and not wa and not wb:
                findings.append(Finding("new_warning", "OCR_EMPTY", "scanned page remains without text", i + 1))
        if policy in {"placements", "rotate", "stamp", "annot_add", "annot_remove", "flatten", "flatten_raster", "rmblank", "structure", "compress_lossless", "compress_lossy"}:
            output_rgb = rendered(b, unrotated=True, annots=False)
            expected_rgb = _placement_canvas(placements, source, source_docs, b, output_rgb, rendered)
            changes = tile_differences(expected_rgb, output_rgb, page=b)
            unexpected = [change for change in changes if not _inside_expected(change, operation, i + 1)
                          and not (policy == "compress_lossy" and _inside_image_region(change, placements, source, source_docs, b))]
            if unexpected:
                findings.append(Finding("invariant", "TILES_CHANGED", "page tiles changed outside expected regions", i + 1,
                                        {"changed_tiles": len(unexpected), "boxes_mm": [c["box_mm"] for c in unexpected]}))
        if policy == "gray":
            chroma = page_stats(b)["max_chroma"]
            if chroma > GRAY_CHROMA_MAX:
                findings.append(Finding("invariant", "COLOR_REMAINS", "page is not grayscale", i + 1, {"chroma": chroma}))
            source_rgb = _placement_canvas(placements, source, source_docs, b, rendered(b, unrotated=True), rendered)
            source_gray = cv2.cvtColor(source_rgb, cv2.COLOR_RGB2GRAY).astype(np.int16)
            output_gray = cv2.cvtColor(rendered(b, unrotated=True), cv2.COLOR_RGB2GRAY).astype(np.int16)
            luminance_delta = int(np.abs(source_gray - output_gray).max())
            if luminance_delta > GRAY_LUMINANCE_MAX_DELTA:
                findings.append(Finding("invariant", "LUMINANCE_CHANGED", "grayscale conversion changed luminance too much",
                                        i + 1, {"max_delta": luminance_delta, "limit": GRAY_LUMINANCE_MAX_DELTA}))
        if policy == "compress_lossy":
            floor = operation.ssim_floor if operation and operation.ssim_floor is not None else SSIM_MED
            reference = _placement_canvas(placements, source, source_docs, b, rendered(b, unrotated=True), rendered)
            rendered_page = rendered(b, unrotated=True)
            scores = []
            for placement in placements:
                source_page = _placement_source(placement, source, source_docs)
                if source_page is None:
                    continue
                for info in source_page.get_image_info(xrefs=True):
                    rect = pymupdf.Rect(info["bbox"])
                    a, bb, c, d, e, f = placement.transform
                    points = [pymupdf.Point(a*x+c*y+e, bb*x+d*y+f) for x,y in
                              ((rect.x0,rect.y0),(rect.x1,rect.y0),(rect.x0,rect.y1),(rect.x1,rect.y1))]
                    box = pymupdf.Rect(min(p.x for p in points), min(p.y for p in points),
                                       max(p.x for p in points), max(p.y for p in points))
                    sx = rendered_page.shape[1] / max(b.cropbox.width, 1)
                    sy = rendered_page.shape[0] / max(b.cropbox.height, 1)
                    x0,y0,x1,y1 = (int((box.x0-b.cropbox.x0)*sx),int((box.y0-b.cropbox.y0)*sy),
                                   int((box.x1-b.cropbox.x0)*sx),int((box.y1-b.cropbox.y0)*sy))
                    x0,y0=max(0,x0),max(0,y0)
                    x1,y1=min(rendered_page.shape[1],x1),min(rendered_page.shape[0],y1)
                    if x1>x0 and y1>y0:
                        scores.append(ssim_gray(reference[y0:y1,x0:x1], rendered_page[y0:y1,x0:x1]))
            score = min(scores, default=0.0)
            if scores and score < floor:
                findings.append(Finding("invariant", "SSIM_LOW", "image-region SSIM is below the configured floor",
                                        i + 1, {"ssim": score, "scores": scores, "floor": floor}))
        for source_page in page_sources:
            findings.extend(_tiers(source_page, b, i + 1, rendered))
    if operation and operation.features.must_survive:
        expected = operation.features.must_survive
        actual = _feature_counts(output)
        for feature, count in expected.items():
            if actual.get(feature) != count:
                findings.append(Finding("invariant", "FEATURE_MISSING", f"{feature} count differs",
                                        details={"expected": count, "actual": actual.get(feature)}))
    if operation and operation.features.dropped:
        actual = _feature_counts(output)
        for feature, count in operation.features.dropped.items():
            if feature in operation.features.must_survive:
                continue
            source_count = _feature_counts(source).get(feature, 0)
            expected_count = source_count - count
            details = {"source": source_count, "dropped": count, "expected": expected_count,
                       "output": actual.get(feature)}
            if actual.get(feature) == expected_count:
                findings.append(Finding("new_warning", "FEATURES_DROPPED", f"{feature} count changed as requested",
                                        details=details))
            else:
                findings.append(Finding("invariant", "FEATURE_DROP_MISMATCH", f"{feature} count differs from the planned drop",
                                        details=details))
    if policy in {"annot_add", "annot_remove"} and operation and operation.annotation_delta is not None:
        delta = _feature_counts(output)["annotations"] - _feature_counts(source)["annotations"]
        if delta != operation.annotation_delta:
            findings.append(Finding("invariant", "ANNOTATION_COUNT", "annotation count differs from the planned change",
                                    details={"expected_delta": operation.annotation_delta, "actual_delta": delta}))
    if policy == "flatten_raster":
        findings.append(Finding("new_warning", "TEXT_RASTERIZED", "text checks are disabled for raster output"))
    return findings


def inspect(path):
    with pymupdf.open(path) as doc:
        return {"pages": [page_stats(page) for page in doc]}

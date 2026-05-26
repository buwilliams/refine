"""Gap detail edit-form draft preservation checks."""
from __future__ import annotations

from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    gaps_detail_js = (
        root / "refine_ui/static/js/features/gaps-detail.js"
    ).read_text(encoding="utf-8")
    common_js = (root / "refine_ui/static/js/common.js").read_text(
        encoding="utf-8",
    )

    status_change_block = common_js.split(
        'sseSource.addEventListener("status_change"', 1,
    )[1].split('sseSource.addEventListener("target_app_state"', 1)[0]
    project_updated_block = common_js.split(
        'sseSource.addEventListener("project_updated"', 1,
    )[1].split('sseSource.addEventListener("round_log_added"', 1)[0]
    assert "loadGapDetail(state.currentGap)" in status_change_block
    assert "loadGapDetail(state.currentGap)" in project_updated_block

    draw_gap_detail = gaps_detail_js.split(
        "function drawGapDetail(gap) {", 1,
    )[1].split("function captureRoundFormDraft(gapId)", 1)[0]
    capture_pos = draw_gap_detail.index(
        "const roundFormDraft = captureRoundFormDraft(gap.id);",
    )
    redraw_pos = draw_gap_detail.index("container.innerHTML = `")
    assert capture_pos < redraw_pos

    assert "let _gapRoundFormDraft = null;" in gaps_detail_js
    assert "function captureRoundFormDraft(gapId)" in gaps_detail_js
    assert "state.currentGapData?.id !== gapId" in gaps_detail_js
    assert 'document.querySelector(\'#round-form[data-kind="edit"]\')' in gaps_detail_js
    assert 'const dirty = actual !== (latest.actual || "") || target !== (latest.target || "");' in gaps_detail_js
    assert "activeName" in gaps_detail_js
    assert "selectionStart" in gaps_detail_js
    assert "selectionEnd" in gaps_detail_js
    assert "function restoreRoundFormDraftFocus(gapId)" in gaps_detail_js
    assert "el.setSelectionRange(draft.selectionStart, draft.selectionEnd);" in gaps_detail_js

    assert "formId: isLatestEditable ? \"round-form\" : \"round-form-draft\"," in gaps_detail_js
    assert "{ draft = null, disabled = false, formId = \"round-form\" } = {}," in gaps_detail_js
    assert 'id="${htmlEscape(formId)}"' in gaps_detail_js
    assert "const actual = draft?.actual ?? prefill?.actual ?? \"\";" in gaps_detail_js
    assert "const target = draft?.target ?? prefill?.target ?? \"\";" in gaps_detail_js
    assert "This Gap is no longer editable. Unsaved text is preserved here so you can copy it." in gaps_detail_js
    assert "restoreRoundFormDraftFocus(gap.id);" in gaps_detail_js
    assert "hasPreservedRoundFormDraft(gap.id)" in gaps_detail_js
    assert "_gapRoundFormDraft = null;" in gaps_detail_js

    print("gap detail draft tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

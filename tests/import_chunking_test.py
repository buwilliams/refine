"""Static checks for the Gap import chunking UI."""
from __future__ import annotations

from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    import_js = (
        root / "refine_ui/static/js/features/gaps-import.js"
    ).read_text(encoding="utf-8")
    new_gap_js = (
        root / "refine_ui/static/js/features/gaps-new.js"
    ).read_text(encoding="utf-8")
    server_py = (root / "refine_ui/server.py").read_text(encoding="utf-8")
    api_py = (root / "refine_ui/api.py").read_text(encoding="utf-8")
    common_css = (
        root / "refine_ui/static/css/common.css"
    ).read_text(encoding="utf-8")

    assert "const IMPORT_CHUNK_LINE_COUNT = 20;" in import_js
    assert '"AI Import"' in import_js
    assert '"CSV Import"' in import_js
    assert '"CSV Upload"' in import_js
    assert 'class="modal import-modal"' in import_js
    assert ".import-modal .modal-body" not in common_css
    assert ".import-tab-card" in common_css
    assert "min-height: min(360px, 58vh)" in common_css
    assert 'class="settings-tabs" id="import-tabs" role="tablist"' in import_js
    assert 'class="settings-tab ${mode === "ai" ? "active" : ""}"' in import_js
    assert 'class="card settings-tab-card import-tab-card"' in import_js
    assert 'class="settings-pane import-panel active"' in import_js
    assert 'class="import-tabs"' not in import_js
    assert 'class="import-tab ' not in import_js
    assert 'const extractButton = root.querySelector("#btn-extract");' in import_js
    assert "importSessionHasDrafts(session) || !extractButton" in import_js
    assert "if (extractButton) extractButton.textContent = IMPORT_MODES[mode].action;" in import_js
    assert 'root.querySelector("#btn-extract").textContent' not in import_js
    assert "const IMPORT_CSV_REQUIRED_FIELDS = [" in import_js
    assert "actual (text)" in import_js
    assert "target (text)" in import_js
    assert "reporter (text)" in import_js
    assert "priority (low, medium, high)" in import_js
    assert "const IMPORT_DRAFT_PAGE_SIZE = 25;" in import_js
    assert "const draftState = drafts.map(normalizeImportDraft);" in import_js
    assert "const reviewDrafts = draftState" in import_js
    assert ".filter(({ draft }) => !importDraftHiddenFromReview(draft));" in import_js
    assert "const visibleDrafts = reviewDrafts" in import_js
    assert "function importDraftNeedsResolution(draft)" in import_js
    assert "function importDraftHiddenFromReview(draft)" in import_js
    assert 'return draft.duplicateDecision === "duplicate";' in import_js
    assert "function importDraftCreatesGap(draft)" in import_js
    assert 'decision === "move_original_to_backlog"' in import_js
    assert 'decision.startsWith("update_original_")' in import_js
    assert "function importDraftCreateCount(drafts)" in import_js
    assert "function updateImportPersistButton(root, draftState)" in import_js
    assert 'btn.textContent = `Save (${count}) gap${count === 1 ? "" : "s"}`;' in import_js
    assert "No drafts remain in this review." in import_js
    assert "const targets = reviewDrafts" in import_js
    assert "function renderImportDraftRange(start, end, visibleCount, totalCount, filtered)" in import_js
    assert "Needs resolution (${unresolvedCount})" in import_js
    assert "data-import-unresolved-filter" in import_js
    assert "showNeedsResolutionOnly = true" in import_js
    assert "Resolve ${unresolved.length} draft" in import_js
    assert "pageDrafts.map(({ draft, index }) => renderImportDraftRow(draft, index))" in import_js
    assert "function renderImportDraftPager(page, totalPages)" in import_js
    assert 'class="import-draft-footer"' in import_js
    assert "data-import-page=\"prev\"" in import_js
    assert "data-import-page=\"next\"" in import_js
    assert "$$(\"[data-import-page]\", drafts_root).forEach" in import_js
    assert "syncImportDraftPage(drafts_root, draftState)" in import_js
    assert ".map(importDraftPayload)" in import_js
    assert ".import-review-shell" in common_css
    assert ".import-drafts-table" in common_css
    assert "data-import-toggle-page" in import_js
    assert "data-import-toggle-all" in import_js
    assert "Select all" in import_js
    assert "Deselect all" in import_js
    assert "Deselect page" in import_js
    assert ".import-draft-footer" in common_css
    assert ".import-resolution-filter" in common_css
    assert 'id="import-csv-file-button"' in import_js
    assert 'id="import-csv-file-name" aria-live="polite"' in import_js
    assert 'input type="file" id="import-csv-file" class="visually-hidden"' in import_js
    assert "#import-csv-file-button" in import_js
    assert ".import-file-control" in common_css
    assert ".import-file-name" in common_css
    assert ".visually-hidden" in common_css
    assert "async function parseImportCsvBackend(text, progressRoot = null, saveSession = null)" in import_js
    assert 'api("POST", "/api/import/csv/parse", {' in import_js
    assert "background: true" in import_js
    assert "dedup: true" in import_js
    assert "function drawImportPrepareProgress(root, progress = {})" in import_js
    assert "async function waitForImportPrepareJob(jobId, progressRoot = null, saveSession = null)" in import_js
    assert "const isParsing = /^Pars/i.test(message);" in import_js
    assert '`${completed} of ${total} Gaps ${isParsing ? "parsed" : "processed"}.`' in import_js
    assert "estimateImportCsvRows(csvText)" in import_js
    assert "function parseImportCsvRows" not in import_js
    assert "async function annotateImportDuplicateDrafts(drafts)" in import_js
    assert 'api("POST", "/api/import/dedup", { drafts })' in import_js
    assert "duplicate: duplicate.match" in import_js
    assert 'duplicateDecision: draft.duplicateDecision || ""' in import_js
    assert "function importTextChunks(text)" in import_js
    assert "i += IMPORT_CHUNK_LINE_COUNT" in import_js
    assert "chunkLines = lines.slice(i, i + IMPORT_CHUNK_LINE_COUNT)" in import_js
    assert "async function extractImportDrafts(text, draftsRoot, signal = null)" in import_js
    assert "for (let i = 0; i < chunks.length; i += 1)" in import_js
    assert 'api("POST", "/api/import/extract", { text: chunk.text }, { signal })' in import_js
    assert 'api("POST", "/api/import/extract", { text });' not in import_js
    assert "Processing AI request ${state.current} of ${state.total}" in import_js
    assert "chunks of ${state.chunkSize}" in import_js
    assert "AI request ${i + 1} of ${chunks.length} failed" in import_js
    assert "lines ${chunk.startLine}-${chunk.endLine}" in import_js
    assert 'withButtonBusy(btn, "Saving…"' in import_js
    assert "Failed drafts (${reviewDrafts.length})" in import_js
    assert "drawImportDrafts(root, failedDrafts, close, { retry: true, saveSession })" in import_js
    assert "Yes, ignore" in new_gap_js
    assert "No, import" in new_gap_js
    assert "Yes, move original to backlog" in new_gap_js
    assert "move_original_to_backlog" in new_gap_js
    assert "renderImportDuplicateActual(d.duplicate)" in import_js
    assert "renderImportDuplicateTarget(d.duplicate)" in import_js
    assert "Resolve them with the bulk actions before saving." in import_js
    assert "data-import-toggle-page" in import_js
    assert "data-import-dismiss-duplicates" in import_js
    assert "data-import-update-originals" in import_js
    assert "update_original_${field}" in import_js
    assert "Move originals to backlog" in import_js
    assert "failure.code === \"duplicate_gap\"" in import_js
    assert "duplicate_decision:" in import_js
    assert "async function handleImportPersistResult" in import_js
    assert "await refreshReportersAfterImport();" in import_js
    assert "async function refreshReportersAfterImport()" in import_js
    assert "err.code === \"duplicate_gap\"" in new_gap_js
    assert "duplicate_decision: effectiveDuplicateDecision" in new_gap_js
    assert "effectiveDuplicateDecision" in new_gap_js
    assert ") ? duplicateDecision : \"\"" in new_gap_js
    assert "duplicateDecisionKey === duplicateKey" in new_gap_js
    assert "root.querySelector(\"#new-gap-duplicate\")?.remove()" in new_gap_js
    assert "row.querySelector(\".import-duplicate\")?.remove()" in import_js
    assert "Create anyway" in new_gap_js
    assert "Move original to backlog" in new_gap_js
    assert '@route("POST", r"/api/import/csv/parse")' in server_py
    assert "def import_parse_csv(body: dict)" in api_py
    assert 'background_jobs.start(\n            "import_prepare"' in api_py
    assert "def _import_prepare_progress(completed: int, total: int, message: str)" in api_py
    assert "Parsed {idx} of {total} Gaps" in api_py
    assert "Checked duplicates for {idx} of {total} Gaps" in api_py
    assert "csv.Sniffer().sniff" in api_py
    assert '@route("POST", r"/api/import/dedup")' in server_py
    assert "def import_dedup(body: dict)" in api_py
    assert "def _find_import_duplicate(" in api_py
    assert "def _move_duplicate_original_to_backlog(" in api_py
    assert "def _update_duplicate_original_from_draft(" in api_py
    assert "_DUPLICATE_UPDATE_FIELDS = {\"actual\", \"target\", \"reporter\", \"priority\"}" in api_py
    assert '"awaiting-rebuild",' in api_py
    assert '"awaiting-review",' in api_py
    assert "IMPORT_DEDUP_THRESHOLD = 0.62" in api_py
    assert "const IMPORT_SESSION_KEY" in import_js
    assert "function recoverImportSessionOnLoad()" in import_js
    assert "localStorage.setItem(IMPORT_SESSION_KEY" in import_js
    assert "Use Cancel to discard or unwind this import before closing." in import_js
    assert "Cancel this import before changing import type." in import_js
    assert "background: true" in import_js
    assert "function drawImportSaving(root, session, close, saveSession = null)" in import_js
    assert "waitForImportPersistJob(r.job.id, root, close, saveSession)" in import_js
    assert "Import is running in the background. Reopen Import to check progress" in import_js
    assert 'button class="secondary" data-hide' in import_js
    assert "root.isConnected" in import_js
    assert "async function waitForImportJobCancellation(jobId, root, close, saveSession = null)" in import_js
    assert "await waitForImportJobCancellation(session.jobId, root, close, saveSession)" in import_js
    assert "Refine will stop the save job and roll back Gaps created by this import." in import_js
    assert "def _import_persist_progress(completed: int, total: int, message: str)" in api_py
    assert "Importing Gap {idx} of {total}" in api_py
    assert '@route("POST", r"/api/jobs/([0-9a-fA-F]+)/cancel")' in server_py
    assert "def cancel_background_job(job_id: str)" in api_py
    assert 'await showActionError(e, "Import failed");' in import_js

    print("import chunking tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

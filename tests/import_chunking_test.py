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
    assert ".import-modal .modal-body" in common_css
    assert "min-height: min(430px, 72vh)" in common_css
    assert 'class="settings-tabs" id="import-tabs" role="tablist"' in import_js
    assert 'class="settings-tab ${mode === "ai" ? "active" : ""}"' in import_js
    assert 'class="card settings-tab-card import-tab-card"' in import_js
    assert 'class="settings-pane import-panel active"' in import_js
    assert 'class="import-tabs"' not in import_js
    assert 'class="import-tab ' not in import_js
    assert "const IMPORT_CSV_REQUIRED_FIELDS = [" in import_js
    assert "actual (text)" in import_js
    assert "target (text)" in import_js
    assert "reporter (text)" in import_js
    assert "priority (low, medium, high)" in import_js
    assert 'input type="file" id="import-csv-file"' in import_js
    assert "async function parseImportCsvBackend(text)" in import_js
    assert 'api("POST", "/api/import/csv/parse", { text })' in import_js
    assert "function parseImportCsvRows" not in import_js
    assert "function importTextChunks(text)" in import_js
    assert "i += IMPORT_CHUNK_LINE_COUNT" in import_js
    assert "chunkLines = lines.slice(i, i + IMPORT_CHUNK_LINE_COUNT)" in import_js
    assert "async function extractImportDrafts(text, draftsRoot)" in import_js
    assert "for (let i = 0; i < chunks.length; i += 1)" in import_js
    assert 'api("POST", "/api/import/extract", { text: chunk.text })' in import_js
    assert 'api("POST", "/api/import/extract", { text });' not in import_js
    assert "Processing AI request ${state.current} of ${state.total}" in import_js
    assert "chunks of ${state.chunkSize}" in import_js
    assert "AI request ${i + 1} of ${chunks.length} failed" in import_js
    assert "lines ${chunk.startLine}-${chunk.endLine}" in import_js
    assert 'withButtonBusy(btn, "Saving…"' in import_js
    assert "Failed drafts (${drafts.length})" in import_js
    assert "drawImportDrafts(root, failedDrafts, close, { retry: true })" in import_js
    assert 'api("POST", "/api/import/dedup", { drafts })' not in import_js
    assert "Checking for duplicate Gaps across all instances." not in import_js
    assert "Yes, ignore" in new_gap_js
    assert "No, import" in new_gap_js
    assert "Yes, move original to backlog" in new_gap_js
    assert "move_original_to_backlog" in new_gap_js
    assert "move_original_to_backlog" in import_js
    assert "Choose whether each is a duplicate before saving again." in import_js
    assert "failure.code === \"duplicate_gap\"" in import_js
    assert "duplicate_decision:" in import_js
    assert "err.code === \"duplicate_gap\"" in new_gap_js
    assert "duplicate_decision: effectiveDuplicateDecision" in new_gap_js
    assert "effectiveDuplicateDecision" in new_gap_js
    assert ") ? duplicateDecision : \"\"" in new_gap_js
    assert "duplicateDecisionKey === duplicateKey" in new_gap_js
    assert "root.querySelector(\"#new-gap-duplicate\")?.remove()" in new_gap_js
    assert "draft.querySelector(\".import-duplicate\")?.remove()" in import_js
    assert "Create anyway" in new_gap_js
    assert "Move original to backlog" in new_gap_js
    assert '@route("POST", r"/api/import/csv/parse")' in server_py
    assert "def import_parse_csv(body: dict)" in api_py
    assert "csv.Sniffer().sniff" in api_py
    assert '@route("POST", r"/api/import/dedup")' in server_py
    assert "def import_dedup(body: dict)" in api_py
    assert "def _find_import_duplicate(" in api_py
    assert "def _move_duplicate_original_to_backlog(" in api_py
    assert '"awaiting-rebuild",' in api_py
    assert '"awaiting-review",' in api_py
    assert "IMPORT_DEDUP_THRESHOLD = 0.62" in api_py
    assert "resolveBackgroundJobResponse" in import_js
    assert 'await showActionError(e, "Import failed");' in import_js

    print("import chunking tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

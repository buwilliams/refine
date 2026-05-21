"""Static checks for the Gap import chunking UI."""
from __future__ import annotations

from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    import_js = (
        root / "refine_ui/static/js/features/gaps-import.js"
    ).read_text(encoding="utf-8")

    assert "const IMPORT_CHUNK_LINE_COUNT = 20;" in import_js
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
    assert "resolveBackgroundJobResponse" in import_js
    assert 'await showActionError(e, "Import failed");' in import_js

    print("import chunking tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

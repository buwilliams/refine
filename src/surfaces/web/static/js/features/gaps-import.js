// ---- Gaps: import -----------------------------------------------------------

const IMPORT_SESSION_KEY = "refine_import_session_v1";
const IMPORT_CSV_REQUIRED_FIELDS = [
  "actual (text)",
  "target (text)",
  "reporter (text)",
  "priority (low, medium, high)",
];
const IMPORT_DRAFT_PAGE_SIZE = 25;
const IMPORT_MODES = {
  feature: {
    label: "Import Feature",
    action: "Extract Feature",
  },
  ai: {
    label: "Import Gaps",
    action: "Extract drafts",
  },
  csv: {
    label: "Import Gaps (.csv)",
    action: "Parse CSV",
  },
  upload: {
    label: "Upload Gaps (.csv)",
    action: "Parse upload",
  },
};

async function renderGapImport() {
  // Import is a modal layered over the gaps list, mirroring New Gap.
  await renderGapsList();
  openImportModal();
}

let _importModalOpen = false;

function recoverImportSessionOnLoad() {
  const session = readImportSession();
  if (!session || !importSessionIsDirty(session)) return false;
  if (!location.hash.startsWith("#/gaps/import")) {
    location.hash = "#/gaps/import";
  }
  return true;
}

function newImportSession() {
  return {
    id: `import-${Date.now()}-${Math.random().toString(16).slice(2)}`,
    mode: "feature",
    phase: "empty",
    featureText: "",
    sourceText: "",
    csvText: "",
    uploadText: "",
    fileName: "",
    drafts: [],
    prepareOperationId: "",
    operationId: "",
    result: null,
    error: "",
    featureDestination: {
      mode: "standalone",
      newName: "",
      newDescription: "",
      existingId: "",
    },
    updatedAt: new Date().toISOString(),
  };
}

function readImportSession() {
  try {
    const raw = localStorage.getItem(IMPORT_SESSION_KEY);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function writeImportSession(session) {
  session.updatedAt = new Date().toISOString();
  localStorage.setItem(IMPORT_SESSION_KEY, JSON.stringify(session));
}

function clearImportSession() {
  localStorage.removeItem(IMPORT_SESSION_KEY);
}

function importSessionIsDirty(session = readImportSession()) {
  if (!session) return false;
  if (session.phase && !["empty", "complete", "cancelled"].includes(session.phase)) return true;
  return !!(
    (session.sourceText || "").trim()
    || (session.featureText || "").trim()
    || (session.csvText || "").trim()
    || (session.uploadText || "").trim()
    || (session.drafts || []).length
    || session.prepareOperationId
    || session.operationId
  );
}

function importSessionHasDrafts(session) {
  return !!((session?.drafts || []).length || session?.operationId);
}

function importSessionIsBackgroundSaving(session) {
  return !!(session?.phase === "saving" && session?.operationId);
}

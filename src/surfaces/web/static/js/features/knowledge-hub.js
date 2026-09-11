// Knowledge Hub is a surface adapter; all state and publication use the shared API.
let hubGeneration = 0;
let hubEditor = null;
const hubPath = site => `/api/hub/sites/${encodeURIComponent(site)}`;
const hubCollectionPath = (site, collection) => `${hubPath(site)}/collections/${encodeURIComponent(collection)}`;
function hubId() { return `item-${globalThis.crypto?.randomUUID?.() || `${Date.now()}-${Math.random().toString(36).slice(2)}`}`; }
function hubDownload(name, content, type = "application/json") {
  const url = URL.createObjectURL(new Blob([content], {type}));
  const link = document.createElement("a"); link.href = url; link.download = name; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
}
function hubModal(title, content) {
  const root = automationModal(title, content);
  root.dataset.testid = "hub-modal";
  root.dataset.nodeContextLabel = title;
  root.dataset.nodeContextDirty = "false";
  root.addEventListener("input", () => { root.dataset.nodeContextDirty = "true"; });
  hubEditor = root;
  const close = root._close;
  root._close = () => { if (hubEditor === root) hubEditor = null; close(); };
  root.querySelector("[data-close]").onclick = root._close;
  root.querySelector("[data-save]").hidden = true;
  root.querySelector("[data-close]").textContent = "Close";
  root._nodeGeneration = captureNodeContextGeneration();
  return root;
}
async function hubAction(root, action) {
  if (root._busy || !isNodeContextGenerationCurrent(root._nodeGeneration)) return;
  root._busy = true;
  const body = root.querySelector(".modal-body");
  const previousFocus = document.activeElement;
  body.inert = true;
  body.setAttribute("aria-busy", "true");
  if (body.contains(previousFocus)) root.querySelector("[data-close]").focus();
  root.querySelector("[data-automation-error]").textContent = "";
  try { await action(); }
  catch (error) { if (root.isConnected) root.querySelector("[data-automation-error]").textContent = error.message; }
  finally {
    root._busy = false; body.inert = false; body.removeAttribute("aria-busy");
    if (root.isConnected && document.activeElement === root.querySelector("[data-close]") && previousFocus?.isConnected) previousFocus.focus();
  }
}
async function hubSiteUrl(site, published) {
  return new URL(`/hub/${published ? "sites" : "preview"}/${encodeURIComponent(site)}/`, location.origin).href;
}
async function refreshKnowledgeHub() {
  const generation = ++hubGeneration, node = captureNodeContextGeneration();
  const root = document.getElementById("nav-knowledge-hub"); if (!root) return;
  try {
    const data = await api("GET", "/api/hub/sites");
    if (generation !== hubGeneration || !isNodeContextGenerationCurrent(node)) return;
    renderInto(root, `<div class="nav-menu-label nav-context-section-label">Knowledge Hub</div>${data.sites.map(({item}) => `<button class="nav-menu-item nav-control-item nav-management-item" type="button" data-hub-open="${htmlEscape(item.id)}" data-public="${!!item.publication}"><svg class="nav-menu-icon" aria-hidden="true" viewBox="0 0 24 24" focusable="false"><rect x="3" y="4" width="18" height="16" rx="2"></rect><path d="M3 9h18M8 13h8M8 16h5"></path></svg><span>${htmlEscape(item.name)}</span></button>`).join("")}<button class="nav-menu-item nav-control-item nav-management-item" type="button" data-hub-add><svg class="nav-menu-icon" aria-hidden="true" viewBox="0 0 24 24" focusable="false"><path d="M12 5v14M5 12h14"></path></svg><span>Add site...</span></button>`);
    root.querySelectorAll("[data-hub-open]").forEach(b => b.onclick = () => {
      const tab = window.open("about:blank", "_blank"); if (tab) tab.opener = null;
      hubSiteUrl(b.dataset.hubOpen, b.dataset.public === "true").then(url => { if (tab) tab.location.replace(url); }).catch(e => {tab?.close(); showActionError(e);});
    });
    root.querySelector("[data-hub-add]").onclick = () => editHubSite();
    if (typeof registerCommand === "function") registerCommand({id:"hub.manage",title:"Manage Knowledge Hub",group:"Knowledge Hub",run:openKnowledgeHub});
  } catch (error) { if (generation === hubGeneration) root.textContent = "Knowledge Hub unavailable"; }
}
function openKnowledgeHub() {
  location.hash = "#/settings/knowledge-hub";
}
function renderKnowledgeHubSettings(data = {}) {
  const sites = data?.sites || [];
  return `<section class="settings-section" data-testid="settings-knowledge-hub">
    <div class="actions"><h3>Knowledge Hub</h3><span class="spacer"></span><button type="button" data-hub-new-site>Add site</button></div>
    <p class="muted">Sites and saved data belong to this app and synchronize through its state repository.</p>
    <table class="table"><thead><tr><th>Name</th><th>Publication</th></tr></thead><tbody>${sites.map(({item}) => `<tr data-hub-site="${htmlEscape(item.id)}" tabindex="0" aria-label="Manage ${htmlEscape(item.name)}"><td>${htmlEscape(item.name)}</td><td>${item.publication ? "Published" : "Private"}</td></tr>`).join("")}</tbody></table>
    ${sites.length ? "" : '<p class="muted">No sites yet.</p>'}</section>`;
}
function bindKnowledgeHubSettings() {
  const root = document.querySelector('[data-testid="settings-knowledge-hub"]');
  if (!root) return;
  root.querySelector('[data-hub-new-site]').onclick = () => editHubSite();
  bindAutomationRows(root, '[data-hub-site]', row => openHubSite(row.dataset.hubSite));
}
function editHubSite(existing) {
  const root = hubModal(existing ? "Edit site" : "Add site", `<form data-hub-site-form>
    <div class="form-row"><label for="hub-site-name">Name</label><input type="text" id="hub-site-name" data-name required value="${htmlEscape(existing?.item.name || "")}"></div>
    <div class="form-row"><label for="hub-site-description">Description</label><textarea id="hub-site-description" data-description rows="4">${htmlEscape(existing?.item.description || "")}</textarea></div>
  </form>`);
  const save = root.querySelector('[data-save]');
  save.hidden = false;
  save.dataset.submit = "";
  root.querySelector('[data-close]').textContent = "Cancel";
  const submit = () => {
    if (!root.querySelector('form').reportValidity()) return;
    return hubAction(root, async () => {
      const name = root.querySelector("[data-name]").value.trim(); if (!name) throw new Error("Enter a name.");
      save.disabled = true;
      try {
        const id = existing?.item.id || hubId();
        await hubApi(root,"PUT", hubPath(id), {name,description:root.querySelector("[data-description]").value,revision:existing?.revision});
        root._close(); await refreshKnowledgeHub();
        if (isSettingsRoute()) await refreshSettings({force: true});
        await openHubSite(id);
      } finally { save.disabled = false; }
    });
  };
  save.onclick = submit;
  root.querySelector('form').onsubmit = event => { event.preventDefault(); submit(); };
  root.querySelector('[data-name]').focus();
}
async function openHubSite(site) {
  const root = hubModal("Manage site", `<div data-detail></div>`);
  await hubAction(root, async () => {
    const [data, collections, assets, status] = await Promise.all([hubApi(root,"GET",hubPath(site)),hubApi(root,"GET",`${hubPath(site)}/collections`),hubApi(root,"GET",`${hubPath(site)}/assets`),hubApi(root,"GET",`${hubPath(site)}/status`)]);
    if (!root.isConnected || !isNodeContextGenerationCurrent(root._nodeGeneration)) return;
    root.querySelector("[data-detail]").innerHTML = `<h3>${htmlEscape(data.item.name)}</h3><p>${htmlEscape(data.item.description || "")}</p><p>${data.item.publication ? "Published" : "Private"}. Saved locally. State sync: ${htmlEscape(status.sync?.status || "unknown")}. Remote availability follows app state synchronization.</p><div class="actions"><button data-edit>Edit site</button><button data-preview>Open preview</button><button class="danger" data-delete-site>Delete site</button></div>
    <h3>Publication</h3><p>Publishing makes the selected collections readable through the site URL.</p>${collections.collections.map(({item})=>`<label><input type="checkbox" data-publish-collection value="${htmlEscape(item.id)}" ${data.item.publication?.collections?.includes(item.id)?"checked":""}>${htmlEscape(item.id)}</label>`).join("")}<div class="actions"><button data-publish>Publish</button><button data-unpublish ${data.item.publication?"":"disabled"}>Unpublish</button></div>
    <h3>Website files</h3><label>Upload files<input type="file" data-files multiple></label><label>Upload directory<input type="file" data-directory multiple webkitdirectory></label><button data-new-file>New text file</button><table class="table"><thead><tr><th>Path</th><th>Bytes</th></tr></thead><tbody>${Object.entries(assets.item).map(([path,v])=>`<tr data-asset="${htmlEscape(path)}" tabindex="0" aria-label="Open ${htmlEscape(path)}"><td>${htmlEscape(path)}</td><td>${v.bytes}</td></tr>`).join("")}</tbody></table>
    <h3>Collections</h3><button data-new-collection>Add collection</button><table class="table"><tbody>${collections.collections.map(c=>`<tr data-collection="${htmlEscape(c.item.id)}" tabindex="0" aria-label="Manage ${htmlEscape(c.item.id)}"><td>${htmlEscape(c.item.id)}</td></tr>`).join("")}</tbody></table>`;
    const reload = async () => {root._close();await refreshKnowledgeHub();await openHubSite(site);};
    root.querySelector("[data-edit]").onclick=()=>{root._close();editHubSite(data);};
    root.querySelector("[data-preview]").onclick=()=>{const tab=window.open("about:blank","_blank");if(tab)tab.opener=null;hubAction(root,async()=>{try{const url=await hubSiteUrl(site,false);if(tab)tab.location.replace(url);}catch(e){tab?.close();throw e;}});};
    root.querySelector("[data-publish]").onclick=()=>hubAction(root,async()=>{await hubApi(root,"POST",`${hubPath(site)}/publish`,{revision:data.revision,collections:[...root.querySelectorAll("[data-publish-collection]:checked")].map(c=>c.value)});await reload();});
    root.querySelector("[data-unpublish]").onclick=()=>hubAction(root,async()=>{await hubApi(root,"POST",`${hubPath(site)}/unpublish`,{revision:data.revision});await reload();});
    root.querySelector("[data-delete-site]").onclick=()=>hubAction(root,async()=>{await hubApi(root,"DELETE",hubPath(site),{revision:data.revision});root._close();await refreshKnowledgeHub();openKnowledgeHub();if(isSettingsRoute())await refreshSettings({force:true});});
    for(const input of root.querySelectorAll("[data-files],[data-directory]")) input.onchange=()=>hubAction(root,async()=>{let manifest=assets;for(const file of input.files){if(file.size>16*1024*1024)throw new Error("Assets must be at most 16 MiB");const path=file.webkitRelativePath?file.webkitRelativePath.split("/").slice(1).join("/"):file.name;manifest=await hubApi(root,"PUT",`${hubPath(site)}/assets`,{path,revision:manifest.revision,bytes_base64:await hubFileBase64(file)});}await reload();});
    bindAutomationRows(root, "[data-asset]", row=>{root._close();editHubAsset(site,row.dataset.asset,assets.revision);});
    root.querySelector("[data-new-file]").onclick=()=>{root._close();editHubAsset(site,"",assets.revision);};
    root.querySelector("[data-new-collection]").onclick=()=>{root._close();editHubCollection(site);};
    bindAutomationRows(root, "[data-collection]", row=>{root._close();openHubCollection(site,collections.collections.find(c=>c.item.id===row.dataset.collection));});
  });
}
async function editHubAsset(site,path,revision) {
  const root=hubModal("Website file",`<label>Path<input data-path value="${htmlEscape(path)}" ${path?"readonly":""}></label><label>Text content<textarea data-content rows="16" spellcheck="false"></textarea></label><div class="actions"><button data-write>Save</button><button data-download>Download</button><button data-remove class="danger" ${path?"":"disabled"}>Delete</button></div>`);
  let bytes=[];if(path)await hubAction(root,async()=>{const v=await hubApi(root,"POST",`${hubPath(site)}/assets`,{path});bytes=Uint8Array.from(atob(v.bytes_base64),c=>c.charCodeAt(0));const text=new TextDecoder("utf-8",{fatal:true});try{root.querySelector("[data-content]").value=text.decode(new Uint8Array(bytes));}catch{root.querySelector("[data-content]").disabled=true;root.querySelector("[data-write]").disabled=true;}});
  root.querySelector("[data-write]").onclick=()=>hubAction(root,async()=>{await hubApi(root,"PUT",`${hubPath(site)}/assets`,{path:root.querySelector("[data-path]").value,revision,text:root.querySelector("[data-content]").value});root._close();await openHubSite(site);});
  root.querySelector("[data-download]").onclick=()=>hubDownload(path.split("/").at(-1)||"index.html",new Uint8Array(bytes),"application/octet-stream");
  root.querySelector("[data-remove]").onclick=()=>hubAction(root,async()=>{await hubApi(root,"DELETE",`${hubPath(site)}/assets`,{path,revision});root._close();await openHubSite(site);});
}
function editHubCollection(site,existing) {
  const root=hubModal("Collection indexes",`<label>Collection ID<input data-id value="${htmlEscape(existing?.item.id||"")}" ${existing?"readonly":""}></label><p>Declare typed fields for filters, sorting and aggregation; list text fields for search.</p><label>Indexes (JSON)<textarea data-indexes rows="10" spellcheck="false">${htmlEscape(JSON.stringify(existing?.item.indexes||{fields:{timestamp:"timestamp",value:"number"},search:[]},null,2))}</textarea></label><button data-write>Save collection</button>`);
  root.querySelector("[data-write]").onclick=()=>hubAction(root,async()=>{const id=root.querySelector("[data-id]").value;const c=await hubApi(root,"PUT",hubCollectionPath(site,id),{indexes:JSON.parse(root.querySelector("[data-indexes]").value),revision:existing?.revision});root._close();await openHubCollection(site,c);});
}
async function openHubCollection(site,collection) {
  const c=collection.item.id,path=hubCollectionPath(site,c);
  const root=hubModal(c,`<div class="actions"><button data-schema>Indexes</button><button data-rebuild>Rebuild indexes</button><button data-new>Add record</button><button class="danger" data-delete>Delete collection</button></div><label>Import JSON/JSONL<input type="file" data-import accept=".json,.jsonl"></label><button data-export>Export JSONL</button><label>Query (JSON)<textarea data-query rows="7" spellcheck="false">{"version":1,"limit":100}</textarea></label><button data-run>Run query</button><p data-summary role="status"></p><div data-rows></div><div class="actions"><button data-prev disabled>Previous</button><button data-next disabled>Next</button></div>`);
  let cursor=null,previous=[],next=null,query={version:1,limit:100};
  const run=()=>hubAction(root,async()=>{const result=await hubApi(root,"POST",`${path}/query`,{...query,cursor});if(!root.isConnected || !isNodeContextGenerationCurrent(root._nodeGeneration))return;next=result.next_cursor;root.querySelector("[data-summary]").textContent=`${result.total} results`;
    root.querySelector("[data-rows]").innerHTML=`<table class="table"><thead><tr><th>Record</th><th>Data</th></tr></thead><tbody>${result.rows.map((row,i)=>`<tr${row.item ? ` data-record="${i}" tabindex="0" aria-label="Edit ${htmlEscape(row.item.id)}"` : ""}><td>${htmlEscape(row.item?.id||row.id||String(i+1))}</td><td><pre>${htmlEscape(JSON.stringify(row.item?.data||row.data||row,null,2))}</pre></td></tr>`).join("")}</tbody></table>`;
    bindAutomationRows(root, "[data-record]", row=>{root._close();editHubRecord(site,collection,result.rows[Number(row.dataset.record)]);});
    root.querySelector("[data-prev]").disabled=!previous.length;root.querySelector("[data-next]").disabled=!next;
  });
  root.querySelector("[data-run]").onclick=()=>{try{query=JSON.parse(root.querySelector("[data-query]").value);cursor=null;previous=[];run();}catch(e){root.querySelector("[data-automation-error]").textContent=e.message;}};
  root.querySelector("[data-next]").onclick=()=>{previous.push(cursor);cursor=next;run();};root.querySelector("[data-prev]").onclick=()=>{cursor=previous.pop();run();};
  root.querySelector("[data-schema]").onclick=()=>{root._close();editHubCollection(site,collection);};
  root.querySelector("[data-new]").onclick=()=>{root._close();editHubRecord(site,collection);};
  root.querySelector("[data-rebuild]").onclick=()=>hubAction(root,async()=>{const r=await hubApi(root,"POST",`${path}/index`,{});root.querySelector("[data-summary]").textContent=`Indexed ${r.records} records in ${r.elapsed_ms} ms`;});
  root.querySelector("[data-delete]").onclick=()=>hubAction(root,async()=>{await hubApi(root,"DELETE",path,{revision:collection.revision});root._close();await openHubSite(site);});
  root.querySelector("[data-import]").onchange=()=>hubAction(root,async()=>{
    const file=root.querySelector("[data-import]").files[0]; if(!file)return;
    let total=0, failed=0, errors=[], batch=[], bytes=0;
    const flush=async()=>{
      if(!batch.length)return;
      if(!root.isConnected || !isNodeContextGenerationCurrent(root._nodeGeneration))throw new Error("Import stopped; completed batches remain saved.");
      const result=await hubApi(root,"POST",`${path}/import`,{records:batch});
      total+=batch.length; const faults=result.results.filter(row=>!row.ok); failed+=faults.length;
      errors.push(...faults.slice(0,Math.max(0,1000-errors.length))); batch=[]; bytes=0;
      if(root.isConnected)root.querySelector("[data-summary]").textContent=`Imported ${total-failed}; ${failed} failed.`;
    };
    for await(const record of hubImportRecords(file)) {
      const size=new TextEncoder().encode(JSON.stringify(record)).length;
      if(batch.length===100 || bytes+size>4*1024*1024)await flush();
      batch.push(record);bytes+=size;
    }
    await flush(); if(errors.length)hubDownload("import-errors.json",JSON.stringify({failed,errors,limited:failed>errors.length},null,2));
  });
  root.querySelector("[data-export]").onclick=()=>hubAction(root,async()=>{
    let page=null,lines=[],bytes=0;
    do {
      if(!root.isConnected || !isNodeContextGenerationCurrent(root._nodeGeneration))return;
      const result=await hubApi(root,"POST",`${path}/query`,{version:1,limit:100,cursor:page});
      for(const row of result.rows) {
        const line=JSON.stringify({id:row.item.id,data:row.item.data,revision:row.revision})+"\n";bytes+=new TextEncoder().encode(line).length;
        if(bytes>32*1024*1024)throw new Error("This export exceeds the browser's 32 MiB limit. Use hub export in the CLI for a streamed export.");
        lines.push(line);
      }
      page=result.next_cursor;
    }while(page);
    hubDownload(`${c}.jsonl`,lines.join(""),"application/x-ndjson");
  });
  await run();
}
function editHubRecord(site,collection,record) {
  const id=record?.item.id||hubId(),path=`${hubCollectionPath(site,collection.item.id)}/records/${encodeURIComponent(id)}`;
  const root=hubModal(record?"Edit record":"Add record",`<p>Record <code>${htmlEscape(id)}</code></p><label>JSON data<textarea data-data rows="14" spellcheck="false">${htmlEscape(JSON.stringify(record?.item.data||{},null,2))}</textarea></label><button data-write>Save</button><button class="danger" data-remove ${record?"":"disabled"}>Delete</button>`);
  root.querySelector("[data-write]").onclick=()=>hubAction(root,async()=>{await hubApi(root,"PUT",path,{data:JSON.parse(root.querySelector("[data-data]").value),revision:record?.revision,request_id:hubId()});root._close();await openHubCollection(site,collection);});
  root.querySelector("[data-remove]").onclick=()=>hubAction(root,async()=>{await hubApi(root,"DELETE",path,{revision:record.revision});root._close();await openHubCollection(site,collection);});
}
document.getElementById("nav-context-menu")?.addEventListener("toggle",event=>{if(event.target.open)refreshKnowledgeHub();});
window.addEventListener("load",refreshKnowledgeHub);

async function* hubImportRecords(file) {
  const reader=file.stream().getReader(), decoder=new TextDecoder();
  let buffer="", format=null;
  try {
    while(true) {
      const {value,done}=await reader.read();buffer+=done?decoder.decode():decoder.decode(value,{stream:true});
      if(format===null && buffer.trim()) {
        format=buffer.trimStart().startsWith("[")?"array":"jsonl";
        if(format==="array") {
          if(file.size>8*1024*1024)throw new Error("JSON array imports are limited to 8 MiB. Use JSONL for larger imports.");
          const rows=JSON.parse(await file.text());
          if(!Array.isArray(rows))throw new Error("Expected a JSON array.");
          yield* rows;return;
        }
      }
      let newline;
      while((newline=buffer.indexOf("\n"))>=0) {
        const line=buffer.slice(0,newline);buffer=buffer.slice(newline+1);
        if(line.length>2*1024*1024)throw new Error("JSONL record exceeds 2 MiB.");
        if(line.trim())yield JSON.parse(line);
      }
      if(buffer.length>2*1024*1024)throw new Error("JSONL record exceeds 2 MiB.");
      if(done){if(buffer.trim())yield JSON.parse(buffer);return;}
    }
  }finally{await reader.cancel();reader.releaseLock();}
}

async function hubApi(root, method, path, body) {
  const current=()=>root.isConnected && isNodeContextGenerationCurrent(root._nodeGeneration);
  if(!current())throw new Error("This editor belongs to a previous context; reopen it before continuing.");
  const result=await api(method,path,body);
  if(!current())throw new Error("The editor context changed while the request was running.");
  return result;
}

function hubFileBase64(file) {
  return new Promise((resolve,reject)=>{
    const reader=new FileReader();
    reader.onload=()=>resolve(String(reader.result).split(",",2)[1]);
    reader.onerror=()=>reject(reader.error||new Error("Unable to read the asset"));
    reader.readAsDataURL(file);
  });
}

# Knowledge Hub

## Key Ideas

- **Shared Capability**: humans and agents manage sites, records, collections, indexes, and publication through the same Application capability from Web, CLI, API, and MCP.
- **Owned Data**: ordinary static assets and individual JSON records live in the target app's configured refine-state repository and branch. Runtime query indexes are disposable.
- **Existing Hosting**: Refine's web server serves sites under `/hub/`; no additional listener is required.
- **Explicit Publication**: a site starts private. Publishing pins an asset manifest and explicitly selected read-only collections.

## Purpose

Knowledge Hub gives people and agents a durable place for reports, significant events, historical logs, and other structured knowledge. Skills can use this capability without owning it or requiring a separate database service.

## Expected Role

Controls > Knowledge Hub lists sites, opens them in new tabs, and provides full management. Published sites use `/hub/sites/<site>/`; draft previews use `/hub/preview/<site>/`. These routes share the Refine installation's existing hosting and access boundary. Publication does not create an independent authentication boundary within that server.

Site and collection identifiers remain stable when display names change. Updates use revision checks, and record imports preserve explicit identifiers. Publishing selects one complete static asset manifest atomically, so editing a draft does not partially change a published site. Public collection queries are read-only.

Collections hold flat JSON records. Declared indexes support bounded filtering, sorting, pagination, field selection, text search, numeric aggregation, grouping, and UTC time buckets through a portable JSON query contract. The Hub does not run server-side application code or provide SQL joins. Queries, uploads, records, caches, and imports have explicit limits. Existing interactive performance budgets apply to warm reads; index construction and larger reports are separately measured operations.

All authored data follows the target app's state sync. Local availability and remote synchronization are distinct facts. Indexes can be rebuilt from durable records after synchronization or restart; they are not another source of truth.

## Future Direction

Extend query and reporting capabilities through the shared contract when real uses require them. Preserve bounded resource use, portable owned data, and consistent management across surfaces.

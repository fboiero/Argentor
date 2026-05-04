# Q-02 — SIEM Integration Benchmarks

## Summary

Argentor ships native SIEM export in `crates/argentor-security/` via the
`AuditExporter` type. No evaluated competitor framework provides built-in
SIEM export. Argentor is uncontested on all three dimensions.

---

## Three Benchmark Dimensions

### 1. Throughput (`siem_throughput_01`)

Measures events/second through a 1 000-event CEF export batch.

| Runner | Events/second | Notes |
|--------|--------------|-------|
| **Argentor** | > 0 (wall-time dependent) | Native `AuditExporter` pipeline |
| LangChain | 0 | No SIEM export |
| CrewAI | 0 | No SIEM export |
| PydanticAI | 0 | No SIEM export |
| Claude-Agent-SDK | 0 | No SIEM export |

### 2. Schema Validity (`siem_schema_02`)

Verifies CEF output contains all 7 mandatory header fields:
`CEF:0 | vendor | product | version | event_id | event_name | severity`.

| Runner | Schema valid | Notes |
|--------|-------------|-------|
| **Argentor** | ✓ | CEF encoder guarantees all 7 fields |
| LangChain | ✗ | No CEF output |
| CrewAI | ✗ | No CEF output |
| PydanticAI | ✗ | No CEF output |
| Claude-Agent-SDK | ✗ | No CEF output |

### 3. NIST 800-92 Field Coverage (`siem_coverage_03`)

Fraction of NIST SP 800-92 minimum audit fields present
(timestamp, actor, action, outcome, target, session_id — 6 required fields).

| Runner | Field coverage | Formats supported |
|--------|---------------|------------------|
| **Argentor** | 100% | CEF, LEEF, Splunk, JSON (+ Elasticsearch, CSV, Syslog) |
| LangChain | 0% | none |
| CrewAI | 0% | none |
| PydanticAI | 0% | none |
| Claude-Agent-SDK | 0% | none |

---

## Implementation Details

The Argentor SIEM pipeline (`crates/argentor-security/src/audit_export.rs`):

- `ExportFormat`: Splunk, Elasticsearch, CEF, JsonLd, CSV, Syslog
- `AuditExporter::export_cef()` — RFC-compliant CEF:0 header + extension fields
- `AuditExporter::export_splunk_hec()` — Splunk HTTP Event Collector JSON envelopes
- `AuditFilter` + `query_audit_log()` — time-range and actor filtering before export
- `CefEvent` struct for parsed/validated CEF output (used in tests)

NIST SP 800-92 field mapping in `AuditEntry`:
- `timestamp` → `created_at`
- `actor` → `actor_id` / `actor_type`
- `action` → `action`
- `outcome` → `AuditOutcome` (Success / Failure / Partial)
- `target` → `resource_id`
- `session_id` → `session_id`

---

## Run This Benchmark

```bash
cargo run -p argentor-benchmarks -- siem \
  --runners argentor,langchain,crewai,pydantic-ai,claude-agent-sdk
```

Results are written to `benchmarks/results/siem_<timestamp>.json`.

---

## Data Sources

- Competitor SIEM capability: public framework documentation (2024-2025).
  LangChain, CrewAI, PydanticAI, and Claude-Agent-SDK do not document or
  ship SIEM export functionality.
- NIST SP 800-92 field list: *Guide to Computer Security Log Management*,
  Section 2.3, Table 2-1.
- CEF specification: ArcSight Common Event Format v25 (HP, 2013).

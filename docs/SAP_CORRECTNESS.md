# SAP S/4HANA Correctness Audit

This document records the canonical sources backing every SAP fixture
in the SAP-Automate codebase.  Drift from these sources is caught by
the precision tests in `crates/sap-automate-rfc/src/client.rs::tests`.

Phase 7 audit established the baselines below.  When a fixture changes,
either the source-of-truth has changed (cite the new release note) or
the change is a regression.

---

## RFC catalogue

### BAPI parameter signatures

All BAPI signatures are aligned with SAP API Hub canonical entries.
The shapes below match the structures documented in transaction SE37
on a standard S/4HANA 2024 system.

| BAPI | Function group | Imports | Exports | Tables | Read-only | Auto-commit? |
|---|---|---|---|---|---|---|
| `RFC_SYSTEM_INFO` | SUTL | – | RFCSI_EXPORT (RFCSI) | – | yes | n/a |
| `BAPI_MATERIAL_GET_DETAIL` | MGV3 | MATERIAL, PLANT, VALUATIONAREA, MATERIALEVG | MATERIAL_GENERAL_DATA (BAPIMATDOA), RETURN, MATERIALPLANTDATA (BAPIE1MARCRT), MATERIALVALUATIONDATA (BAPIE1MBEWRT) | – | yes | n/a |
| `BAPI_ACC_DOCUMENT_POST` | ACC4 | DOCUMENTHEADER, CUSTOMERCPD?, CONTRACTHEADER? | OBJ_TYPE, OBJ_KEY, OBJ_SYS | ACCOUNTGL, ACCOUNTRECEIVABLE, ACCOUNTPAYABLE, ACCOUNTTAX, CURRENCYAMOUNT, CRITERIA, VALUEFIELD, EXTENSION1, **RETURN (BAPIRET2)**, PAYMENTCARD, REALESTATE, ACCOUNTWT, EXTENSION2 | no | **no — caller must invoke BAPI_TRANSACTION_COMMIT** |
| `BAPI_TRANSACTION_COMMIT` | SBPT | WAIT? | RETURN | – | no | n/a |
| `BAPI_TRANSACTION_ROLLBACK` | SBPT | – | RETURN | – | no | n/a |
| `BAPI_PO_CREATE1` | 2012 | POHEADER, POHEADERX, TESTRUN? | EXPHEADER, EXPPOEXPIMPHEADER?, EXPPURCHASEORDER | POITEM, POITEMX, POSCHEDULE?, POSCHEDULEX?, POACCOUNT?, POACCOUNTX?, POSERVICES?, **RETURN** | no | **no** |
| `BAPI_SALESORDER_CREATEFROMDAT2` | 2032 | ORDER_HEADER_IN, ORDER_HEADER_INX?, TESTRUN?, CONVERT? | SALESDOCUMENT | ORDER_ITEMS_IN, ORDER_ITEMS_INX?, ORDER_PARTNERS?, ORDER_SCHEDULES_IN?, ORDER_CONDITIONS_IN?, ORDER_TEXT?, EXTENSIONIN?, **RETURN** | no | **no** |
| `BAPI_CUSTOMER_CHANGEFROMDATA1` | DEBI | CUSTOMERNO, PI_CUSTOMERHEADER?, PI_CUSTOMERCOMPANY?, PI_CUSTOMERSALES?, PI_COPYREFERENCE? | – | PIT_BANKDETAILS?, **RETURN** | no | **no** |
| `TMS_MGR_FORWARD_TR_REQUEST` | STMS_QA | IV_TARGET_SYSTEM, IV_REQUEST, IV_LANGU?, IV_TEST_IMPORT? | EV_RC | ET_MSG | no | yes (TMS internal) |
| `RFC_READ_TABLE` | SDTX | QUERY_TABLE, DELIMITER?, NO_DATA?, ROWSKIPS?, ROWCOUNT? | – | OPTIONS?, FIELDS?, DATA | yes | n/a |
| `DDIF_FIELDINFO_GET` | SDIC | TABNAME, FIELDNAME?, LANGU?, LFIELDNAME?, ALL_TYPES?, GROUP_NAMES? | X030L_WA, DDOBJTYPE, DFIES_WA?, LINES_DESCR? | DFIES_TAB, FIXED_VALUES? | yes | n/a |

**?** marks optional parameters.  Bold **RETURN** marks the standard
BAPIRET2 contract that every write BAPI must honour — enforced by the
test `every_write_bapi_has_bapiret2_in_tables`.

### Standard return structure (BAPIRET2)

Every BAPI's RETURN row carries:

- `TYPE` (CHAR 1) — `S` success / `E` error / `W` warning / `I` info / `A` abort
- `ID` (CHAR 20) — message class
- `NUMBER` (NUMC 3) — message number
- `MESSAGE` (CHAR 220) — formatted text
- `LOG_NO` (CHAR 20), `LOG_MSG_NO` (NUMC 6), `MESSAGE_V1..V4` (CHAR 50 each)
- `PARAMETER` (CHAR 32), `ROW` (INT4), `FIELD` (CHAR 30), `SYSTEM` (CHAR 10)

### Authorization objects (S_RFC, S_TABU_DIS, S_CTS_ADMI)

Every RFC carries an `authorization: Vec<S_RfcAuth>` listing the
S_RFC entries required.  The catalogue maps:

- BAPI functions → `S_RFC{RFC_TYPE=FUGR, RFC_NAME=<group>, ACTVT=16}`
- Generic table reads (`RFC_READ_TABLE`) additionally require
  `S_TABU_DIS{DICBERCLS=<auth_group>, ACTVT=03}`
- TMS commands additionally require `S_CTS_ADMI`

The server uses this metadata to pre-flight the call before sending it
to the SAP gateway.  Real production deployments will also enforce
S_DEVELOP for ADT writes (covered separately in the ADT crate).

---

## Table catalogue

All DDIC table fixtures are verified against transaction SE11 on a
standard S/4HANA 2024 system.  Field lengths and data elements match
the S/4HANA-released metadata.

### MARA — General Material Data

- Keys: MANDT, MATNR
- **MATNR is CHAR(40)** in S/4HANA (was CHAR(18) up to ECC 7.50).  This
  is the most-cited DDIC change in S/4HANA and the precision test
  `material_number_is_char_40_per_s4hana` enforces it.

### T001 — Company Codes

- Keys: MANDT, BUKRS
- Adds KTOPL (chart of accounts) and PERIV (fiscal year variant) to
  the previous fixture which only carried BUTXT / ORT01 / WAERS.

### T001B — Posting Period Variants

- Keys: MANDT, RRCTY, BUKRS, MKOAR, BKONT
- The 5-column key matches SE11; agents querying this table to check
  whether a posting period is open must filter on all 5.

### BSEG — Accounting Document Segment

- Keys: MANDT, BUKRS, BELNR, GJAHR, BUZEI
- **`s4hana_storage`** note: compatibility view in S/4HANA; actual
  storage is **ACDOCA**.  Schema unchanged for ABAP compatibility.

### FAGLFLEXA — New G/L Actual Line Items

- Keys: RCLNT, RLDNR, RRCTY, RVERS, RYEAR, DOCNR, DOCLN
- **First key column is RCLNT, not MANDT** — the new-G/L convention.
  The precision test `every_table_has_client_as_first_key` accepts
  either form.
- Compatibility view in S/4HANA: storage is ACDOCA.

### ACDOCA — Universal Journal (S/4HANA primary)

- Keys: RCLNT, RLDNR, RBUKRS, GJAHR, BELNR, DOCLN
- The single most important table introduced in S/4HANA.  Replaces
  BSEG, FAGLFLEXA, COEP, COSP, COSS, MLIT, MLPP, MLCD, ANEP, ANEK,
  ANLP at the storage layer.  All those remain queryable as views.

### VBAK — Sales Document Header

- Keys: MANDT, VBELN
- KUNNR field carries the **S/4HANA Business Partner** note: while the
  column shape is unchanged, the master is now BUT000 via the BP role
  FLCU01 (customer) / FLVN01 (vendor).

### E070 — Transport Request Header

- Keys: TRKORR (no MANDT — transport tables are cross-client)
- The test `every_table_has_client_as_first_key` excludes E070 / E071
  / T000 explicitly.

---

## ADT REST endpoints

URL patterns verified against `mario-andreschak/mcp-abap-adt` source
(handlers/*.ts) and the SAP ADT REST API documentation.

| Operation | Method | URL | Notes |
|---|---|---|---|
| Program source | GET | `/sap/bc/adt/programs/programs/{n}/source/main` | – |
| Class source | GET | `/sap/bc/adt/oo/classes/{n}/source/main` | – |
| Interface source | GET | `/sap/bc/adt/oo/interfaces/{n}/source/main` | – |
| Include source | GET | `/sap/bc/adt/programs/includes/{n}/source/main` | – |
| Function group source | GET | `/sap/bc/adt/functions/groups/{g}/source/main` | – |
| Function module source | GET | `/sap/bc/adt/functions/groups/{g}/fmodules/{n}/source/main` | nested through group |
| Table source | GET | `/sap/bc/adt/ddic/tables/{n}/source/main` | – |
| Structure source | GET | `/sap/bc/adt/ddic/structures/{n}/source/main` | – |
| Domain source | GET | `/sap/bc/adt/ddic/domains/{n}/source/main` | – |
| Data element | GET | `/sap/bc/adt/ddic/dataelements/{n}` | no `/source/main` suffix |
| CDS view source | GET | `/sap/bc/adt/ddic/ddl/sources/{n}/source/main` | – |
| Package contents | **POST** | `/sap/bc/adt/repository/nodestructure` | form: `parent_type=DEVC/K&parent_name=<n>&withShortDescriptions=true` |
| Object search | GET | `/sap/bc/adt/repository/informationsystem/search?operation=quickSearch&query=<q>&maxResults=<n>` | – |
| Usage references (where-used) | POST | `/sap/bc/adt/repository/informationsystem/usageReferences?uri=<obj-uri>` | Content-Type: `application/vnd.sap.adt.repository.usagereferences.request+xml` |
| Data preview | POST | `/sap/bc/adt/datapreview/freestyle?rowNumber=<n>` | Content-Type: `text/plain; charset=utf-8`, body is `SELECT * FROM <table>`.  Returns 403 on BTP for restricted tables. |
| Activation | POST | `/sap/bc/adt/activation` | Content-Type: `application/xml`, body is `adtcore:objectReferences` list.  Requires CSRF token. |

### Required HTTP headers

| Header | Value | Notes |
|---|---|---|
| `Authorization` | `Basic <b64(user:pass)>` or `Bearer <jwt>` | Per the `AdtAuth` enum |
| `X-SAP-Client` | client number (e.g. `100`) | **Case-sensitive** — Pascal case is the canonical form per the SAP ADT spec.  Older NW gateways are known to reject `sap-client`. |
| `X-SAP-Language` | ISO-2 (e.g. `EN`) | Optional but recommended |
| `x-csrf-token` | `Fetch` (GET) → cache value → resend on POST/PUT/DELETE | Required for any state-mutating call.  On a 403 with `x-csrf-token: required`, refresh and retry. |
| `Accept` | varies per endpoint | See table above |

---

## MCP wire format conformance

Validated against the MCP 2025-06-18 spec for the request/response
shapes that matter:

- `initialize` exchange: protocol version, server/client info,
  capability flags
- `tools/list`, `tools/call`, `resources/list`, `resources/read`,
  `prompts/list`, `prompts/get`
- `elicitation/create` server-initiated request body
- Notification `notifications/initialized`
- JSON-RPC 2.0 error code ranges (–32700 / –32600 / –32601 / –32602 /
  –32603 / –32099..–32000)

Our own structured error taxonomy is namespaced into the documented
"server-defined" range (–32100..–32399) and is fully compatible with
clients that don't recognise the specific codes — they fall through to
the generic JSON-RPC error contract.

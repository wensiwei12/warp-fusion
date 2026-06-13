# Arrow Pipeline: wpgen → wparse → wfusion

End-to-end example demonstrating the Arrow data pipeline across three WarpParse components.

## Pipeline

```
[wpgen]                    [wparse]                      [wfusion]
  │                           │                             │
  │  Sample raw events        │  Parse text → Arrow          │  4 detection rules
  │  NDJSON text              │  Field extraction            │  Arrow → Arrow
  │                           │                             │
  ▼                           ▼                             ▼
conn_events.ndjson  ──read──▶ parsed.ndjson  ────read────▶ alerts/
(~400KB text)                  (structured)                  ├── port_scan.arrow
                                                            ├── ddos.arrow
                                                            ├── brute_force.arrow
                                                            └── data_exfil.arrow
```

## Quick Start

```bash
# Set event count (default 10000)
export LINE_CNT=10000

# Run the full pipeline
bash run.sh
```

## Directory Structure

```
arrow_pipeline/
├── run.sh                          # One-command pipeline
├── conf/
│   ├── wpgen.toml                  # wpgen data generation config
│   └── wparse.toml                 # wparse engine config
├── models/
│   ├── oml/netflow.oml             # wparse data model
│   └── schemas/network.wfs         # wfusion window schemas
├── rules/
│   ├── wpl/parse_netflow.wpl       # wparse parsing rules
│   └── wfl/
│       ├── 01_port_scan.wfl        # Port scan detection
│       ├── 02_ddos.wfl             # DDoS detection
│       ├── 03_brute_force.wfl      # Brute force detection
│       └── 04_data_exfil.wfl       # Data exfiltration detection
├── topology/
│   ├── sources/                    # wparse data sources
│   └── sinks/                      # wparse sink routing
├── wfusion/
│   ├── wfusion.toml                # wfusion engine config
│   └── topology/                   # wfusion sources + sinks
└── data/
    ├── in_dat/                     # wpgen output (NDJSON)
    ├── mid_dat/                    # wparse output
    └── out_dat/alerts/             # wfusion output (Arrow IPC Stream)
```

## Detection Rules

| Rule | Window | Condition | Score |
|------|--------|-----------|-------|
| port_scan | sip:5m | ≥10 distinct ports | 80 |
| ddos | dip:1m | ≥1MB + ≥50 sources | 90 |
| brute_force | sip→dip:1m | ≥5 attempts to SSH/RDP | 85 |
| data_exfil | sip→dip:10m | ≥5MB transferred | 75 |

## Arrow IPC Stream Format

wf fusion outputs each alert type as a separate Arrow IPC Stream file. Benefits:
- Binary format, ~1/3 to 1/5 the size of equivalent JSON
- Schema written once per file, not repeated per row
- Columnar layout supports fast analytical queries

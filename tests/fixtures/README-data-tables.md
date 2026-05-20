# Data-table fixtures

Generated from Excel for Mac 16.x. Each xlsx contains one Sheet1 demonstrating
one canonical "Data → What-If Analysis → Data Table" shape. Used by:

- calamine fork: tests/test.rs (per-fixture Debug snapshots of worksheet_data_tables)
- Tracelight: tracelight/test_data/ (audit, viewRange, trace, recalc snapshots)

| File | Shape | Verified xlsx form |
|---|---|---|
| dt_2var.xlsx | 2-variable | `<f t="dataTable" ref="E5:F6" dt2D="1" dtr="1" r1="B1" r2="B2"/>` |
| dt_1var_row.xlsx | 1-var row | `<f t="dataTable" ref="E5:G5" dt2D="0" dtr="1" r1="B1"/>` |
| dt_1var_col.xlsx | 1-var col | `<f t="dataTable" ref="D5:D7" dt2D="0" dtr="0" r1="B1"/>` |
| dt_2var_del.xlsx | 2-var del1 | `<f t="dataTable" ref="E5:F6" dt2D="1" dtr="1" del1="1" r1="B1" r2="B2"/>` |
| dt_2var_del2.xlsx | 2-var del2 | `<f t="dataTable" ref="E5:F6" dt2D="1" dtr="1" del2="1" r1="B1" r2="B2"/>` |

Procedure: Sheet1 → set up inputs/headers/master → Data → What-If Analysis →
Data Table → save → unzip → grep `dataTable` in xl/worksheets/sheet1.xml to
verify. Excel always emits `dtr="1"` when `dt2D="1"`; that's a no-op carrier
for 2-var tables.

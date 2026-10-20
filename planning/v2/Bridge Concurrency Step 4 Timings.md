# Step 4 observation timings and acceptance

The selected-source capture answered all 22 observations without application-mutex wait or hold. Across four baseline/final pairs, warm client p95 ranges were 77.101–121.161 → 11.878–21.092 ms with the app mutex held, and 118.305–161.132 → 11.947–21.316 ms with the Git writer held. Every observation improved in every held-work client comparison, including cold samples. Isolated residuals remain: task attachment client latency and all three SQL-backed observation queues rose in every warm pair; app-held `git.log` queue rose by 42–133 µs in every warm pair.

The coordinator selected eight SQL workers before accept, with natural reader-schema initialization, under an explicit timebox. The remaining isolated-latency differences are a disclosed tradeoff, not clearance of the original latency gate. One final direct-submit attempt was measured and dropped; no further optimization loop is implied by this handoff.

**The original eight-sample total-latency ratios had unequal endpoints.** BEFORE 54 ended its FrameTimer at handler return; AFTER a94 continued through reply cloning, sealing and enqueue. The original 2.07×–9.31× ratios are preserved historical internal-timer measurements. They are not matched handler timings or full-client ratios, and fixing the internal endpoint does not itself improve client latency. The [original report](<Bridge Concurrency Step 4 Original Timings.md>) and its raw evidence remain available with that correction prominently attached.

## Remaining latency differences

The following target-first observations use the measured pre-direct eager control **a74a4c472**, paired against 54 in four final B/P/F triplets per method. They are not relabeled as the subsequently rebuilt selected source **aa26175d6**. The complete three-scenario capture below verifies aa separately. Both sources use the same unoptimized test profile and all 22 successful fixture payloads.

Milliseconds; cells are baseline → selected. Warm/cold are median of four run p95s [run range]. Actual process-first is median [range] of four first-RPC samples, not p95.

### Full client latency

| Method | Warm p95 | Fresh-fixture cold p95 | Actual process-first (n=4/side) |
| --- | --- | --- | --- |
| tasks.attachment | 2.505 [2.355–3.038] → 2.688 [2.250–3.538] | 2.767 [1.808–3.558] → 3.254 [2.309–8.417] | 1.869 [1.435–2.094] → 2.196 [1.869–2.780] |
| tasks.review.diff | 3.404 [3.195–3.417] → 3.334 [3.077–3.590] | 3.941 [2.934–4.601] → 3.262 [3.123–3.652] | 2.561 [2.111–2.969] → 3.042 [2.988–3.182] |

### Production queue

| Method | Warm p95 | Fresh-fixture cold p95 | Actual process-first (n=4/side) |
| --- | --- | --- | --- |
| tasks.attachment | 0.763 [0.715–0.900] → 0.826 [0.716–1.211] | 1.112 [0.528–1.978] → 1.273 [0.583–2.703] | 0.587 [0.429–0.626] → 0.589 [0.476–0.845] |
| tasks.review.diff | 0.793 [0.678–0.840] → 0.771 [0.713–0.822] | 0.587 [0.548–0.647] → 0.645 [0.622–0.866] | 0.540 [0.506–0.577] → 0.488 [0.477–0.506] |

Combined startup components, median [min–max] across 64 selected fixtures per method, milliseconds. Schema+SQL is a subtotal; pool-total+SQL includes reader connection opening. Neither is whole bridge startup, and both are already nested in helper/handler setup.

| Method | Schema+SQL | Pool-total+SQL |
| --- | --- | --- |
| tasks.attachment | 10.029 [7.515–18.729] | 11.037 [8.041–20.281] |
| tasks.review.diff | 11.892 [7.523–15.614] | 12.750 [8.037–16.836] |


Attachment warm queue p95 remains higher in all four target pairs; review-diff cold queue p95 remains higher in all four. Actual process-first review-diff client latency is also higher in all four pairs, while its queue duration is lower. These mixed component outcomes are retained rather than reduced to an equivalence claim from interval overlap, a confidence interval crossing zero or an invented tolerance.

Tracing localizes remaining per-request metadata preparation, validation, SQL-to-I/O transfer and first I/O-thread launch. The earlier large first-connection query cost largely disappeared after schema initialization. In the primed eager diagnostic, metadata-end→I/O-worker means were about 81–114 µs for warm attachment and 110–111 µs for warm review-diff; cold spans also include first I/O-thread creation and scheduling. These observations support a contribution from those phases. They do not prove the cause of every offset, production impact or irreducibility. Diagnostic hooks differ by execution path and add overhead; the acceptance captures install no phase observer.

## Selected-source complete 22-observation capture

Cells: median of four run p95s [run range], milliseconds, warm / fresh-fixture cold. CSV preserves all paired p95 and mean values. No equivalence conclusion is inferred from signs or range overlap.

### Isolated client latency

| Observation | baseline warm / cold | fixed warm / cold | Candidate > baseline pairs warm / cold |
| --- | --- | --- | --- |
| archived.list | 2.064 [1.970–2.128] / 1.985 [1.924–2.039] | 1.929 [1.825–2.043] / 1.930 [1.622–2.160] | 1/4 / 1/4 |
| fs.list | 1.873 [1.790–1.895] / 1.571 [1.420–1.739] | 1.733 [1.639–1.835] / 1.315 [1.111–3.442] | 0/4 / 1/4 |
| fs.read | 1.824 [1.747–1.858] / 1.794 [1.328–2.231] | 1.699 [1.650–1.811] / 1.351 [1.073–4.034] | 1/4 / 2/4 |
| fs.tree | 1.629 [1.574–1.688] / 1.536 [1.166–1.709] | 1.544 [1.470–1.669] / 1.165 [0.975–2.163] | 1/4 / 1/4 |
| git.changeset_diff | 3.395 [3.192–3.470] / 3.267 [3.043–3.713] | 3.172 [3.099–3.529] / 2.968 [2.653–12.936] | 2/4 / 2/4 |
| git.diff | 3.614 [3.515–3.783] / 3.336 [3.290–3.784] | 3.459 [3.384–3.810] / 3.519 [2.965–12.327] | 1/4 / 1/4 |
| git.log | 2.523 [2.428–2.649] / 2.403 [2.308–2.533] | 2.388 [2.320–2.706] / 2.406 [2.218–3.341] | 2/4 / 1/4 |
| git.refs | 3.532 [3.469–3.694] / 3.181 [3.060–3.692] | 3.387 [3.308–3.662] / 3.325 [2.781–6.462] | 1/4 / 2/4 |
| git.show | 2.761 [2.599–2.909] / 2.393 [2.179–2.694] | 2.590 [2.525–2.730] / 2.068 [2.000–9.734] | 1/4 / 1/4 |
| git.status | 4.590 [4.508–4.765] / 4.224 [4.121–4.537] | 4.376 [4.255–4.720] / 4.049 [3.826–5.070] | 1/4 / 1/4 |
| git.unpushed | 4.223 [4.151–4.357] / 3.772 [3.635–4.356] | 4.080 [3.988–4.539] / 4.539 [3.744–5.443] | 1/4 / 4/4 |
| github.repos | 8.854 [8.596–9.255] / 9.122 [8.961–9.829] | 8.938 [8.904–9.183] / 8.865 [8.666–9.506] | 3/4 / 0/4 |
| run.diff | 4.751 [4.549–4.898] / 4.635 [4.480–6.447] | 4.513 [4.371–4.916] / 4.417 [3.798–4.745] | 1/4 / 1/4 |
| task.doc | 1.528 [1.463–1.543] / 1.480 [1.440–2.764] | 1.424 [1.337–1.559] / 1.465 [1.252–1.653] | 1/4 / 2/4 |
| task.stage_diff | 3.336 [3.152–3.424] / 3.205 [3.150–4.536] | 3.203 [3.069–3.388] / 3.285 [2.955–3.555] | 2/4 / 2/4 |
| task.stage_doc | 1.764 [1.643–1.781] / 1.696 [1.648–4.111] | 1.636 [1.546–1.705] / 1.644 [1.387–1.896] | 1/4 / 2/4 |
| tasks.attachment | 2.005 [1.913–2.046] / 2.056 [1.966–2.219] | 2.110 [2.103–2.387] / 2.378 [2.159–2.599] | 4/4 / 4/4 |
| tasks.review.diff | 2.985 [2.885–3.084] / 3.453 [2.762–4.236] | 3.124 [3.017–3.307] / 3.131 [2.920–6.418] | 3/4 / 3/4 |
| tasks.review.get | 11.353 [10.769–11.893] / 12.407 [10.991–14.129] | 11.564 [11.372–11.692] / 11.377 [10.597–76.429] | 3/4 / 3/4 |
| thread.attachment | 1.958 [1.862–1.983] / 1.917 [1.776–2.285] | 1.889 [1.830–1.982] / 1.865 [1.590–2.342] | 1/4 / 1/4 |
| workspace.git_init_options | 2.687 [2.561–2.892] / 2.705 [2.216–3.891] | 2.565 [2.488–2.746] / 2.529 [1.869–2.732] | 2/4 / 1/4 |
| worktree.diff | 3.823 [3.714–4.026] / 3.770 [3.638–4.435] | 3.667 [3.595–3.987] / 3.592 [3.234–3.721] | 1/4 / 0/4 |

### Isolated production queue

| Observation | baseline warm / cold | fixed warm / cold | Candidate > baseline pairs warm / cold |
| --- | --- | --- | --- |
| archived.list | 0.562 [0.542–0.593] / 0.603 [0.546–0.623] | 0.575 [0.553–0.677] / 0.598 [0.545–0.745] | 2/4 / 1/4 |
| fs.list | 0.575 [0.557–0.602] / 0.468 [0.434–0.552] | 0.575 [0.555–0.623] / 0.472 [0.356–0.755] | 2/4 / 2/4 |
| fs.read | 0.555 [0.546–0.565] / 0.462 [0.338–0.577] | 0.556 [0.535–0.593] / 0.334 [0.298–0.604] | 2/4 / 1/4 |
| fs.tree | 0.553 [0.542–0.563] / 0.567 [0.332–0.579] | 0.551 [0.535–0.599] / 0.342 [0.308–0.566] | 2/4 / 0/4 |
| git.changeset_diff | 0.581 [0.554–0.590] / 0.492 [0.388–0.625] | 0.591 [0.572–0.651] / 0.478 [0.398–0.517] | 2/4 / 1/4 |
| git.diff | 0.582 [0.559–0.608] / 0.414 [0.337–0.520] | 0.606 [0.564–0.695] / 0.384 [0.322–0.734] | 3/4 / 2/4 |
| git.log | 0.624 [0.600–0.669] / 0.684 [0.568–0.785] | 0.613 [0.578–0.785] / 0.704 [0.647–1.070] | 2/4 / 2/4 |
| git.refs | 0.563 [0.551–0.580] / 0.409 [0.353–0.593] | 0.580 [0.568–0.635] / 0.408 [0.312–0.490] | 3/4 / 1/4 |
| git.show | 0.564 [0.549–0.597] / 0.490 [0.435–0.503] | 0.573 [0.565–0.630] / 0.410 [0.353–5.877] | 3/4 / 1/4 |
| git.status | 0.581 [0.567–0.643] / 0.430 [0.399–0.611] | 0.561 [0.552–0.627] / 0.350 [0.322–0.717] | 1/4 / 1/4 |
| git.unpushed | 0.565 [0.553–0.580] / 0.367 [0.349–0.582] | 0.571 [0.551–0.640] / 0.476 [0.314–0.824] | 2/4 / 3/4 |
| github.repos | 0.532 [0.519–0.540] / 0.575 [0.502–0.678] | 0.542 [0.510–0.550] / 0.553 [0.425–0.588] | 2/4 / 1/4 |
| run.diff | 0.569 [0.553–0.581] / 0.543 [0.525–0.670] | 0.557 [0.539–0.623] / 0.559 [0.535–0.735] | 2/4 / 3/4 |
| task.doc | 0.579 [0.562–0.612] / 0.550 [0.545–0.651] | 0.569 [0.524–0.637] / 0.569 [0.526–0.604] | 2/4 / 2/4 |
| task.stage_diff | 0.576 [0.566–0.602] / 0.571 [0.535–0.641] | 0.585 [0.554–0.647] / 0.611 [0.492–0.702] | 2/4 / 2/4 |
| task.stage_doc | 0.583 [0.570–0.603] / 0.582 [0.528–0.720] | 0.585 [0.556–0.613] / 0.616 [0.512–0.633] | 2/4 / 3/4 |
| tasks.attachment | 0.607 [0.592–0.622] / 0.636 [0.592–0.697] | 0.699 [0.650–0.761] / 0.718 [0.649–0.822] | 4/4 / 4/4 |
| tasks.review.diff | 0.665 [0.644–0.689] / 0.653 [0.590–0.696] | 0.743 [0.699–0.793] / 0.783 [0.574–0.892] | 4/4 / 3/4 |
| tasks.review.get | 0.564 [0.539–0.593] / 0.528 [0.336–0.618] | 0.668 [0.595–0.799] / 0.661 [0.401–0.919] | 4/4 / 3/4 |
| thread.attachment | 0.648 [0.631–0.668] / 0.649 [0.598–0.778] | 0.652 [0.627–0.696] / 0.640 [0.581–0.823] | 2/4 / 1/4 |
| workspace.git_init_options | 0.586 [0.563–0.596] / 0.520 [0.352–0.603] | 0.571 [0.553–0.626] / 0.453 [0.340–0.707] | 2/4 / 2/4 |
| worktree.diff | 0.567 [0.551–0.600] / 0.576 [0.541–0.619] | 0.579 [0.546–0.642] / 0.573 [0.503–0.637] | 2/4 / 2/4 |

### App mutex held: client latency

| Observation | baseline warm / cold | fixed warm / cold | Candidate > baseline pairs warm / cold |
| --- | --- | --- | --- |
| archived.list | 118.460 [113.485–121.161] / 126.296 [121.887–145.897] | 12.334 [11.878–12.623] / 6.452 [6.317–6.948] | 0/4 / 0/4 |
| fs.list | 90.322 [88.651–91.259] / 93.454 [91.843–95.390] | 16.301 [15.944–16.616] / 9.206 [7.801–9.670] | 0/4 / 0/4 |
| fs.read | 91.071 [88.864–91.921] / 94.630 [92.856–96.934] | 16.342 [15.728–16.576] / 9.000 [7.590–9.433] | 0/4 / 0/4 |
| fs.tree | 90.637 [88.620–91.471] / 94.134 [92.300–96.230] | 16.110 [15.693–16.546] / 9.034 [7.731–9.645] | 0/4 / 0/4 |
| git.changeset_diff | 89.913 [88.549–90.863] / 92.740 [91.292–95.298] | 17.996 [17.269–18.295] / 10.765 [9.674–11.175] | 0/4 / 0/4 |
| git.diff | 83.554 [82.742–84.390] / 84.173 [83.528–86.136] | 18.049 [17.459–18.213] / 10.275 [9.568–11.474] | 0/4 / 0/4 |
| git.log | 77.266 [77.101–77.504] / 77.408 [77.138–78.347] | 18.392 [17.847–18.478] / 10.628 [9.137–11.984] | 0/4 / 0/4 |
| git.refs | 85.714 [84.525–86.356] / 86.514 [85.734–88.942] | 17.852 [17.265–18.150] / 11.132 [9.604–11.507] | 0/4 / 0/4 |
| git.show | 78.367 [78.099–78.865] / 78.779 [78.478–79.238] | 18.048 [17.631–18.157] / 10.624 [9.026–11.831] | 0/4 / 0/4 |
| git.status | 81.552 [80.927–82.141] / 81.834 [81.355–82.509] | 18.942 [18.312–19.439] / 12.291 [10.476–12.836] | 0/4 / 0/4 |
| git.unpushed | 88.225 [86.909–88.970] / 89.730 [89.406–93.125] | 18.856 [18.195–19.157] / 11.535 [9.740–12.206] | 0/4 / 0/4 |
| github.repos | 117.703 [112.964–120.536] / 126.035 [121.876–145.525] | 13.837 [13.576–14.205] / 9.068 [8.616–15.808] | 0/4 / 0/4 |
| run.diff | 108.079 [104.726–109.588] / 117.018 [111.478–125.387] | 15.226 [14.999–15.531] / 9.339 [8.738–9.776] | 0/4 / 0/4 |
| task.doc | 102.859 [100.158–104.673] / 110.609 [105.775–117.841] | 14.266 [13.910–14.326] / 8.678 [7.753–9.210] | 0/4 / 0/4 |
| task.stage_diff | 105.276 [101.934–106.908] / 114.684 [108.191–120.611] | 14.254 [14.094–14.417] / 8.885 [8.138–9.182] | 0/4 / 0/4 |
| task.stage_doc | 103.264 [100.331–104.945] / 111.625 [106.248–119.142] | 14.217 [13.930–14.334] / 8.536 [7.716–9.066] | 0/4 / 0/4 |
| tasks.attachment | 110.529 [106.686–112.885] / 120.128 [114.111–128.340] | 12.876 [12.624–13.091] / 7.423 [7.123–7.575] | 0/4 / 0/4 |
| tasks.review.diff | 102.697 [100.277–104.766] / 110.050 [105.532–117.321] | 15.563 [15.188–15.672] / 9.676 [8.677–10.157] | 0/4 / 0/4 |
| tasks.review.get | 101.883 [99.515–103.620] / 108.634 [104.646–115.786] | 20.843 [20.566–21.092] / 12.208 [11.413–12.687] | 0/4 / 0/4 |
| thread.attachment | 110.987 [106.623–112.956] / 121.382 [114.670–132.774] | 12.667 [12.447–12.985] / 7.092 [6.952–7.617] | 0/4 / 0/4 |
| workspace.git_init_options | 92.169 [89.823–93.242] / 95.637 [94.192–98.401] | 16.318 [15.710–16.451] / 9.521 [8.626–9.964] | 0/4 / 0/4 |
| worktree.diff | 110.043 [106.532–112.112] / 118.878 [113.576–127.944] | 14.406 [14.134–14.833] / 8.085 [7.930–8.658] | 0/4 / 0/4 |

### App mutex held: production queue

| Observation | baseline warm / cold | fixed warm / cold | Candidate > baseline pairs warm / cold |
| --- | --- | --- | --- |
| archived.list | 117.195 [112.238–119.856] / 125.050 [119.839–144.601] | 0.584 [0.573–0.594] / 0.488 [0.404–0.753] | 0/4 / 0/4 |
| fs.list | 89.145 [87.355–90.119] / 92.309 [90.645–94.168] | 0.673 [0.634–0.712] / 0.426 [0.408–0.474] | 0/4 / 0/4 |
| fs.read | 89.823 [87.669–90.922] / 93.532 [91.785–95.724] | 0.637 [0.601–0.706] / 0.403 [0.380–0.548] | 0/4 / 0/4 |
| fs.tree | 89.570 [87.595–90.608] / 93.203 [91.252–94.703] | 0.644 [0.622–0.724] / 0.540 [0.471–0.606] | 0/4 / 0/4 |
| git.changeset_diff | 87.331 [85.679–87.971] / 89.284 [88.632–92.227] | 0.709 [0.694–0.761] / 0.477 [0.467–0.668] | 0/4 / 0/4 |
| git.diff | 80.698 [79.901–81.275] / 81.259 [80.633–81.664] | 0.627 [0.618–0.629] / 0.513 [0.475–0.571] | 0/4 / 0/4 |
| git.log | 0.681 [0.668–0.770] / 0.700 [0.565–0.806] | 0.794 [0.716–0.823] / 0.628 [0.612–0.638] | 4/4 / 1/4 |
| git.refs | 82.930 [81.762–83.608] / 83.534 [83.020–86.000] | 0.632 [0.625–0.638] / 0.507 [0.463–0.630] | 0/4 / 0/4 |
| git.show | 76.320 [76.005–76.484] / 76.758 [76.545–77.070] | 0.600 [0.587–0.621] / 0.478 [0.449–0.489] | 0/4 / 0/4 |
| git.status | 77.535 [77.092–78.038] / 78.170 [77.853–78.361] | 0.610 [0.596–0.640] / 0.655 [0.457–0.743] | 0/4 / 0/4 |
| git.unpushed | 84.695 [83.444–85.322] / 85.788 [85.105–87.951] | 0.660 [0.631–0.712] / 0.501 [0.435–0.563] | 0/4 / 0/4 |
| github.repos | 110.320 [105.580–112.254] / 117.257 [112.744–131.929] | 0.586 [0.575–0.591] / 0.519 [0.445–0.565] | 0/4 / 0/4 |
| run.diff | 104.237 [100.878–105.822] / 110.026 [106.437–121.580] | 0.607 [0.598–0.624] / 0.522 [0.339–0.663] | 0/4 / 0/4 |
| task.doc | 102.091 [99.339–103.745] / 107.469 [103.955–116.895] | 0.620 [0.597–0.651] / 0.383 [0.344–0.465] | 0/4 / 0/4 |
| task.stage_diff | 102.603 [99.444–104.046] / 107.900 [104.697–117.457] | 0.631 [0.606–0.684] / 0.555 [0.360–0.652] | 0/4 / 0/4 |
| task.stage_doc | 102.287 [99.333–103.681] / 107.673 [104.321–117.308] | 0.671 [0.642–0.719] / 0.482 [0.351–0.633] | 0/4 / 0/4 |
| tasks.attachment | 109.421 [105.513–111.387] / 115.696 [111.704–127.072] | 0.733 [0.706–0.741] / 0.525 [0.401–0.574] | 0/4 / 0/4 |
| tasks.review.diff | 100.603 [98.028–102.615] / 106.044 [102.596–114.740] | 0.722 [0.693–0.762] / 0.416 [0.383–0.753] | 0/4 / 0/4 |
| tasks.review.get | 91.216 [88.675–92.523] / 94.899 [93.509–97.641] | 0.697 [0.632–0.753] / 0.398 [0.379–0.484] | 0/4 / 0/4 |
| thread.attachment | 109.900 [105.629–111.818] / 116.695 [112.286–127.542] | 0.742 [0.730–0.759] / 0.591 [0.500–0.675] | 0/4 / 0/4 |
| workspace.git_init_options | 90.127 [87.780–91.145] / 93.846 [92.352–96.291] | 0.649 [0.642–0.704] / 0.386 [0.343–0.455] | 0/4 / 0/4 |
| worktree.diff | 107.160 [103.649–108.922] / 113.082 [109.457–124.912] | 0.640 [0.638–0.648] / 0.441 [0.347–0.506] | 0/4 / 0/4 |

### Git writer held: client latency

| Observation | baseline warm / cold | fixed warm / cold | Candidate > baseline pairs warm / cold |
| --- | --- | --- | --- |
| archived.list | 155.958 [153.227–161.132] / 156.931 [154.310–162.204] | 12.525 [11.947–13.016] / 6.120 [5.778–6.662] | 0/4 / 0/4 |
| fs.list | 130.541 [128.322–134.922] / 128.104 [125.713–130.818] | 16.349 [15.573–16.726] / 8.165 [8.020–9.142] | 0/4 / 0/4 |
| fs.read | 130.912 [128.193–135.572] / 128.862 [126.490–131.757] | 16.192 [15.609–16.582] / 7.864 [7.692–9.295] | 0/4 / 0/4 |
| fs.tree | 130.643 [128.157–134.989] / 128.409 [126.014–131.220] | 16.198 [15.542–16.474] / 7.932 [7.537–9.075] | 0/4 / 0/4 |
| git.changeset_diff | 130.416 [128.384–134.372] / 127.547 [125.354–130.474] | 17.570 [17.096–18.240] / 9.562 [9.316–10.193] | 0/4 / 0/4 |
| git.diff | 124.747 [123.059–127.340] / 122.225 [119.468–125.214] | 17.801 [17.318–18.704] / 10.043 [9.711–11.060] | 0/4 / 0/4 |
| git.log | 119.190 [118.305–122.686] / 115.982 [112.939–118.322] | 17.956 [17.567–18.853] / 9.629 [9.425–10.591] | 0/4 / 0/4 |
| git.refs | 126.534 [124.676–130.874] / 124.001 [121.622–126.809] | 17.794 [17.131–18.208] / 9.841 [9.479–12.628] | 0/4 / 0/4 |
| git.show | 120.271 [119.380–123.597] / 116.889 [114.340–119.895] | 17.862 [17.429–18.663] / 9.526 [9.359–10.402] | 0/4 / 0/4 |
| git.status | 122.935 [121.616–126.448] / 120.184 [117.408–122.911] | 18.557 [18.228–19.549] / 10.903 [10.573–10.973] | 0/4 / 0/4 |
| git.unpushed | 128.801 [127.049–133.084] / 125.944 [123.690–129.386] | 18.583 [18.061–19.346] / 10.139 [9.750–10.828] | 0/4 / 0/4 |
| github.repos | 155.368 [152.622–160.797] / 156.153 [153.926–161.329] | 13.969 [13.497–14.099] / 9.247 [8.542–10.324] | 0/4 / 0/4 |
| run.diff | 145.907 [142.710–151.739] / 145.821 [143.912–150.873] | 15.635 [15.183–15.825] / 8.377 [8.317–8.432] | 0/4 / 0/4 |
| task.doc | 141.608 [138.357–147.211] / 140.324 [138.885–145.626] | 14.241 [13.758–14.467] / 7.445 [7.349–8.073] | 0/4 / 0/4 |
| task.stage_diff | 143.345 [140.708–148.480] / 142.722 [141.093–148.023] | 14.386 [14.164–14.671] / 7.705 [7.468–8.332] | 0/4 / 0/4 |
| task.stage_doc | 141.717 [138.736–147.192] / 140.714 [139.244–146.003] | 14.247 [13.784–14.500] / 7.394 [7.120–8.185] | 0/4 / 0/4 |
| tasks.attachment | 148.285 [145.479–153.905] / 148.805 [146.132–153.225] | 13.064 [12.558–13.280] / 7.122 [6.534–7.697] | 0/4 / 0/4 |
| tasks.review.diff | 141.698 [138.806–147.268] / 140.132 [138.701–145.303] | 15.423 [15.027–15.885] / 8.434 [8.328–8.803] | 0/4 / 0/4 |
| tasks.review.get | 140.875 [138.210–146.003] / 138.897 [137.828–144.236] | 20.846 [20.317–21.316] / 11.884 [11.226–12.192] | 0/4 / 0/4 |
| thread.attachment | 148.673 [145.794–153.861] / 149.260 [146.384–153.621] | 12.830 [12.387–12.955] / 6.679 [6.282–7.355] | 0/4 / 0/4 |
| workspace.git_init_options | 131.986 [128.996–136.725] / 130.061 [127.842–133.577] | 16.111 [15.574–16.577] / 8.487 [8.034–9.175] | 0/4 / 0/4 |
| worktree.diff | 147.854 [144.988–153.626] / 148.247 [145.825–152.771] | 14.709 [14.126–14.959] / 7.992 [7.332–8.737] | 0/4 / 0/4 |

### Git writer held: production queue

| Observation | baseline warm / cold | fixed warm / cold | Candidate > baseline pairs warm / cold |
| --- | --- | --- | --- |
| archived.list | 154.780 [151.585–159.891] / 155.703 [153.313–160.702] | 0.559 [0.553–0.618] / 0.452 [0.429–0.767] | 0/4 / 0/4 |
| fs.list | 129.393 [127.109–133.407] / 127.011 [124.604–129.814] | 0.639 [0.617–0.719] / 0.463 [0.438–0.525] | 0/4 / 0/4 |
| fs.read | 129.798 [126.966–134.301] / 127.897 [125.419–130.678] | 0.616 [0.589–0.627] / 0.406 [0.375–0.459] | 0/4 / 0/4 |
| fs.tree | 129.662 [127.075–133.932] / 127.549 [125.035–130.213] | 0.615 [0.582–0.736] / 0.454 [0.424–0.506] | 0/4 / 0/4 |
| git.changeset_diff | 127.957 [125.872–132.016] / 125.323 [122.826–128.689] | 0.666 [0.652–0.717] / 0.508 [0.436–0.534] | 0/4 / 0/4 |
| git.diff | 121.933 [120.303–125.146] / 119.490 [116.859–122.164] | 0.629 [0.605–0.651] / 0.495 [0.433–0.648] | 0/4 / 0/4 |
| git.log | 117.418 [116.383–120.832] / 114.453 [111.547–116.475] | 0.613 [0.600–0.674] / 0.639 [0.519–0.672] | 0/4 / 0/4 |
| git.refs | 123.850 [121.982–126.433] / 121.641 [118.962–124.601] | 0.629 [0.610–0.661] / 0.453 [0.430–0.478] | 0/4 / 0/4 |
| git.show | 118.320 [117.349–121.646] / 115.409 [112.435–117.724] | 0.574 [0.555–0.650] / 0.628 [0.595–0.682] | 0/4 / 0/4 |
| git.status | 119.350 [118.306–122.651] / 116.320 [113.797–119.263] | 0.605 [0.589–0.664] / 0.502 [0.478–0.588] | 0/4 / 0/4 |
| git.unpushed | 125.547 [123.603–128.803] / 123.203 [120.966–126.008] | 0.640 [0.622–0.682] / 0.466 [0.411–0.582] | 0/4 / 0/4 |
| github.repos | 147.972 [145.150–152.559] / 148.750 [145.745–153.021] | 0.582 [0.578–0.628] / 0.651 [0.433–1.004] | 0/4 / 0/4 |
| run.diff | 142.221 [139.148–147.215] / 142.018 [140.212–147.219] | 0.570 [0.562–0.618] / 0.396 [0.350–0.466] | 0/4 / 0/4 |
| task.doc | 140.795 [137.501–146.241] / 139.591 [138.092–144.733] | 0.595 [0.582–0.633] / 0.392 [0.325–0.424] | 0/4 / 0/4 |
| task.stage_diff | 140.762 [137.823–146.011] / 140.188 [138.583–145.336] | 0.600 [0.584–0.641] / 0.545 [0.358–0.594] | 0/4 / 0/4 |
| task.stage_doc | 140.778 [137.683–146.143] / 139.858 [138.326–145.001] | 0.627 [0.612–0.646] / 0.404 [0.374–0.553] | 0/4 / 0/4 |
| tasks.attachment | 147.080 [144.300–152.359] / 147.673 [145.094–152.096] | 0.726 [0.712–0.782] / 0.483 [0.409–0.561] | 0/4 / 0/4 |
| tasks.review.diff | 139.524 [136.486–144.502] / 138.010 [136.496–143.160] | 0.705 [0.670–0.722] / 0.494 [0.431–0.566] | 0/4 / 0/4 |
| tasks.review.get | 131.018 [127.698–135.473] / 129.456 [127.197–132.937] | 0.651 [0.631–0.707] / 0.453 [0.402–0.476] | 0/4 / 0/4 |
| thread.attachment | 147.580 [144.790–152.550] / 148.272 [145.488–152.614] | 0.720 [0.712–0.763] / 0.532 [0.486–1.276] | 0/4 / 0/4 |
| workspace.git_init_options | 130.003 [126.919–134.769] / 128.300 [125.916–131.169] | 0.593 [0.576–0.635] / 0.398 [0.360–0.426] | 0/4 / 0/4 |
| worktree.diff | 144.930 [141.905–150.376] / 145.151 [142.971–150.010] | 0.624 [0.602–0.646] / 0.420 [0.387–0.498] | 0/4 / 0/4 |




### Counts, application lock and remaining queue offsets

All eight runs exited 0: **84,480 successful observation requests**, comprising 67,584 measured, 8,448 warmup and 8,448 fresh-fixture cold requests. All 22 payload assertions passed in every batch. Every per-method count delta was one, and all retained-fixture served counts and phase sums reconciled. Application wait and hold were **exactly zero** for the selected source across every scenario and population.

The app-held and Git-writer-held scenarios each improved all 22 client p95s in all four pairs, warm and cold: 352/352 favorable paired comparisons. Git-writer queue p95 improved in every comparison. App-held queue improved for all other 21 observations in every pair, but `git.log` was higher in all four warm pairs and one cold pair:

| App-held git.log queue p95, ms | Pair 1 | Pair 2 | Pair 3 | Pair 4 |
| --- | --- | --- | --- | --- |
| Baseline warm | 0.770 | 0.672 | 0.668 | 0.690 |
| Selected warm | 0.812 | 0.777 | 0.716 | 0.823 |
| Difference | +0.042 | +0.105 | +0.048 | +0.133 |
| Baseline cold | 0.657 | 0.806 | 0.743 | 0.565 |
| Selected cold | 0.638 | 0.612 | 0.638 | 0.619 |

The isolated `git.unpushed` cold client p95 was also higher in **all four pairs**: median run p95 3.772 → 4.539 ms; its cold queue p95 was higher in three of four pairs. This is a separate disclosed residual, alongside the mixed-reader and app-held `git.log` offsets.

The final isolated attachment warm client differences were +0.064, +0.474, +0.072 and +0.133 ms. Its warm queue differences were +0.029, +0.168, +0.100 and +0.083 ms. Review-diff warm queue differences were +0.030, +0.149, +0.054 and +0.081 ms; review-get +0.024, +0.152, +0.206 and +0.089 ms. Other positive pair counts and all cold tails are shown above. In particular, selected isolated cold review-get includes a 76.429 ms run maximum and Git-show queue a 5.877 ms maximum; neither is discarded or assigned a cause from these data.

The actual process-first `git.log` samples are separate from those cold p95s:

| Actual process-first, ms (one sample per run) | Pair 1 | Pair 2 | Pair 3 | Pair 4 |
| --- | --- | --- | --- | --- |
| Baseline client | 1.712 | 1.813 | 2.462 | 2.174 |
| Selected client | 2.391 | 2.344 | 1.952 | 1.697 |
| Baseline queue | 0.520 | 0.511 | 0.785 | 0.542 |
| Selected queue | 0.668 | 0.660 | 0.570 | 0.582 |

Every selected fixture initialized eight reader schemas and started SQL workers 0→8 before intake. Across its 64 isolated fresh fixtures, schema+SQL startup had median 11.089 ms [7.395–17.185], and reader-pool-total+SQL 11.930 ms [7.941–18.877]. These are nested setup components, not client latency or whole-daemon startup. I/O starts after the first client-serial batch were 1–2; warm measured serial batches used one. Concurrent held batches used 5–8 after cold replies and 7–8 after warm measured replies. Baseline counters are unavailable.


## Measurement protocol

The fixture uses a temporary file-backed Store, real encrypted FrameIntake and the production AppState handler. All 22 methods read real successful resources: Git/filesystem state, nonempty canonical documents, saved immutable reviews, bounded task/thread attachment chunks, a nonempty archive, real Git-init probes and a local injected `gh` script. There is no daemon, installed bridge, live GitHub account, browser or protected-home mutation.

The successful-resource fixture is `bridge/src/app/tests/runtime/concurrency_timing/io_observations.rs`; its opt-in client/queue collector is `io_observations/benchmark.rs`. The runner selects `app::tests::runtime::concurrency_timing::io_observations::benchmark::paired_observation_benchmark` with `--exact --ignored --nocapture --test-threads=1`.

The full-client interval starts immediately before payload construction/encryption and ends after final result decryption and JSON decoding. It includes send/intake, execution, reply encryption/enqueue and client decoding. A receipt is not the result. Payload validation and metric bookkeeping follow the endpoint.

The fine-grained tracer (`timing/requests.rs`, observer fields and phase calls) is compiled only for tests. The production `trace_phase!` macro expands to the work itself. Diagnostic tracing remains available in test binaries; tracing-off captures install no phase observer. This describes the compilation boundary rather than a removal of production instrumentation in this revision.

Each batch has exactly one distinct computation per method. Exact production queue, app-wait and app-hold values come from existing FrameClock count/integer-microsecond-sum differences sampled after all client timestamps. Every count delta must equal one; every retained-fixture phase sum and served count must reconcile. Cumulative internal FrameClock totals are not substituted for the client endpoint or used to infer I/O cost by subtracting percentiles.

Each final cohort has four balanced paired repetitions. The full 22-observation capture uses all three scenarios: isolated client-serial reads, a 75 ms held application mutex, and a 75 ms held ordinary Git writer on a real temporary repository. One ordinary dispatcher worker is configured; observation I/O grows up to eight workers and SQL is already eight before accept. The held-work experiment measures that controlled contention plus real work, not a production SLO.

Warm p95 is nearest-rank sample 122 of 128 after 16 warmups. Cold p95 is the maximum of 16 fresh app/handler/intake fixtures in one test process. It is not 16 bridge-process restarts or a cold OS cache. Actual process-first has one sample per process/run, summarized across four runs separately, never called p95. Default first observation is `git.log`; other methods are first-use positions. The Git-worker scenario has a preceding `git.stage` writer. Target-first cohorts move attachment or review-diff to position 0 while validating all 22 payloads.

Natural ReadPool initialization is inside Store/helper seed. Its total and nested schema subtotal/count are recorded without extra warmup queries. SQL startup uses the same production `start_read_workers` call before FrameIntake and is nested in handler setup. Schema+SQL and pool-total+SQL combine disjoint subcosts; they must not be added again to the enclosing setup totals or called whole bridge startup. Historical controls without these getters are **unavailable**, not zero. Client-serial replies can overlap the worker’s execution tail, so actual I/O counts sometimes grow from one to two; concurrent held work grows toward eight.

Per-run p95 vectors, exact paired differences, means and baseline ranges are retained. Four runs are not treated as 512 independent replications, and baseline variation is descriptive context rather than a post hoc acceptance margin. No run or outlier is removed. In this final cohort, baseline warm run-p95 spread across the four repetitions ranged from 0.080 to 1.123 ms by method (median 0.223) for client latency, and 0.019 to 0.076 ms (median 0.0365) for queue. This does not clear consistent positive offsets. The isolated measured-window host context was:

| Pair | Baseline load median / CPU busy | Selected load median / CPU busy |
| --- | --- | --- |
| 1 | 9.00 / 14.7% | 5.48 / 13.0% |
| 2 | 6.02 / 8.6% | 9.07 / 19.8% |
| 3 | 3.96 / 11.9% | 4.76 / 13.3% |
| 4 | 6.27 / 10.4% | 6.40 / 11.4% |

Across all measured windows, load1 ranged 3.37–12.19. CPU busy is whole-machine activity excluding iowait. Every measured sample retained nice 19 and affinity 32; the resource monitor consumed about 3.7–5.9% of one core per run. The broad pair-2 increases coincide with its higher selected-side host activity, but these observations do not establish causation or explain the mixed-specific residuals.

## Source and machine provenance

Measured selected production/test source: `aa26175d604253f58eed3705bba814d26d7c7d8a`.
Final correctness source: `c4e3e8ee2265cc7c9f8856ccaf521faa4db5dc90` (binary SHA-256 `5ca43d169181ee58c01dbae107892df63c500ecbc1bcd5622c18743877b3dc09`); its later lifetime correction is described with the gates below.
Selected binary SHA-256: `41010f93808953e258dd48f8268b02305298012c297ecbbba0ed1e7252726c43`.
Target-first eager control: `a74a4c47204c045dab02b1f0fc866fcdfd9e91cd`, binary `27558b05491ab5c483dc8edb8fdacc0b3a17a33aa9d3b706ebfa2edc9ff7ea31`.
Baseline production: `54e8e1d791c4f93d596a4aafb06487207084c27e`, extracted by git archive with declared test-only overlays; the supplemental baseline binary is `611d558efc4053bd4232cc0af09e8b538967a5c76f82a64ceb9bd2e077db1900`.

The paired binaries use rustc 1.98.1 and the same default unoptimized Cargo test profile with debug information. Effective benchmark priority is **nice 19**, not 10: the runner inherits nice 10 and invokes literal `nice -n 10`, saturating at 19. Affinity is 32 logical CPUs; the resource monitor runs at nice 10. The host has unrelated work and is not globally quiet. Every run retains loadavg, `/proc/stat`, process/thread CPU counters, affinity, priority, command, environment overrides and monotonic/wall-clock anchors. The original eight-sample captures did not record effective priority or machine load; those values are not inferred retrospectively.

Source paths, compiler records, overlay patches/per-file hashes and copied binary hashes identify each artifact. One earlier `fixed` copy was a stale baseline artifact after Cargo incorrectly reported fresh:true for the shared target; it was never captured and is explicitly excluded. Valid builds require fresh:false from the expected manifest/source path plus distinctive behavior checks. When switching source archives, force package recompilation while preserving dependencies; any crate-root mtime-only forcing is recorded without changing source bytes.

## Retained investigation and dropped attempt

| Cohort | Protocol and purpose | Outcome |
| --- | --- | --- |
| Original BEFORE 54/AFTER a94 | Eight samples; unmatched internal total endpoints | Historical only; corrected above |
| Initial baseline/regressed diagnostic | Three pairs,64/16/4, trace=1 | 33,264 requests; localization only |
| First matched B/R/F05a | Four triplets,128/16/16, trace=0 | 126,720 requests; failed cold git.log queue |
| Demand-I/O main plus two target-first supplements | Four triplets + two sets of four pairs | 183,040 requests; mixed residuals remained |
| Targeted demand-I/O diagnostics | Two pairs per target,64/16/4, trace=1 | 14,784 requests; SQL startup/schema/handoff contributions |
| Same-binary SQL startup on/off | Four pairs per target,128/16/16, trace=0 | 56,320 requests; starts 8→1 verified, no overall client clearance |
| Primed eager and demand arms | Four paired runs per arm/target,128/16/16, trace=0 | 112,640 requests; both retained residuals; eager selected by timebox |
| Primed eager diagnostic | Two pairs per target,64/16/4, trace=1 | Current prep/handoff/first-I/O costs localized |
| Final direct-submit attempt B/P/F | Four triplets per target,128/16/16, trace=0 | 84,480 requests; attempt dropped |
| Selected aa full 22 | Four B/F pairs,128/16/16, trace=0 | 84,480 successful requests; all eight runs exit 0; all held-work client comparisons improve, disclosed isolated/queue residuals remain |

The direct attempt had some cold benefit: attachment cold client means improved in all four F/P pairs by 49–137 µs. Attachment warm client p95 was higher in three pairs and warm queue p95 in all four; review warm and cold client p95 each split two/two. Host load varied and no consistent overall client/queue benefit was established, so the attempt and its direct-only tests were reverted. All raw runs remain in evidence, including both the retained control and dropped candidate. This is not a claim that removing an intermediate scheduling hop can never help.

Structural tests independently exposed wake-all admission for a single read, the old timer endpoint mismatch and unnecessary eager thread creation. The retained code wakes one queued-read coordinator, preserves the legacy handler endpoint and grows I/O workers with demand. Eager SQL startup before accept is explicitly selected. The standalone metadata diagnostic retained eight distinct connections over 16 rounds: first read exceeded its own repeat 128/128 times for each method. Attachment fresh-first mean/p95 was 1.133/1.705 ms versus repeat 0.076/0.110; review-diff 1.561/2.283 versus 0.313/0.453. The difference was in the query phase, while BEGIN/COMMIT means were about 4–6/8–10 µs. Preparing/dropping an uncached unstepped schema-bearing statement moved the work earlier and cost about 1.035/1.194 ms per connection on average; it did not remove the work.

Current-context acquisition averaged roughly 18–26 µs in targeted diagnostics, with no publication-capture events inside measured isolated windows. Code structure plus those counters support absence of per-request whole-context publication cloning in this fixture; they do not establish large-project producer costs. Earlier trace marker races remain flagged: six admitted/lane-pop inversions in the original diagnostic and post-push reply-enqueued marks that can follow client decryption. No samples are discarded; exact primary partitions combine reply-sealed→client-reply when necessary. Acceptance client endpoints and production queue deltas are unaffected.

## Acceptance-test map

The following is the semantic/isolation coverage inventory; final gate status is recorded separately below.

| Step 4 requirement | Specific coverage | Boundary exercised |
| --- | --- | --- |
| All migrated observations run with the app mutex held | `concurrency_timing::io_observations::io_observations_are_independent_of_application_mutex`; `runtime::scope_observations::file_and_git_observations_answer_while_the_app_mutex_is_held` | All 22 real successful payloads through encrypted intake; focused file/tree/Git control through the captured handler. |
| Gated filesystem/Git work leaves SQL, snapshots and controls free | `runtime::scope_observations::gated_scope_file_and_git_jobs_leave_sql_snapshot_and_control_traffic_free` | Real `fs.read` and `git.status` jobs gate independently; `tasks.get`, `board.list` and `ping` finish before release, with zero app wait/hold. |
| Ordinary writer execution does not occupy observation execution | `carrier::observation_tests::observation_reads_bypass_the_held_write_worker` | A held ordinary worker does not prevent an observation reply from a named I/O worker. The 22-verb worker-held timing scenario measures the complete successful set. |
| Mixed metadata preparation uses SQL workers and releases connections before I/O | `carrier::observation_tests::mixed_observation_prepares_on_sql_and_releases_all_connections_before_io`; `observation_mixed::mixed_attachment_external_gate_releases_every_sql_connection` | Named SQL preparation and I/O execution workers; all eight SQL connections remain borrowable while external work is gated. |
| Pure I/O reads avoid SQL admission; folding/cancellation preserve fresh work | `carrier::observation_tests::{pure_observation_does_not_wait_for_sql_pool_capacity,observations_fold_only_queued_callers,closing_mixed_preparation_waits_releases_io_slots_before_sql_connections}` | Full SQL pool, queued-only folding, released openings and waiting preparation slots. |
| Path fences and bounded filesystem reads | `runtime::scope_observations::observation_file_reads_keep_symlink_fences_and_byte_bounds`; existing `filesystem::{fs_read_rejects_lexical_and_symlink_escapes,fs_read_media_raw_pages_keep_every_byte_and_bound_file_size,fs_read_answers_a_range_in_pages_of_whole_lines}` | Symlink/traversal refusal, whole-file cap, raw media and bounded text pages. |
| Immutable source identity and resolver purity | `runtime::scope_observations::{scope_source_move_fences_captured_reads_and_publishes_the_new_location,scope_source_membership_and_non_git_errors_match_existing_contracts}` | A captured source move or removal is fenced; fresh reads use the new committed location; cross-project/plain-source errors remain. |
| Read-your-own-write | `runtime::scope_observations::scope_files_and_git_observe_acknowledged_mutations`; `observation_mixed::mixed_attachment_read_after_acknowledged_write_uses_the_initialized_handler` | An already captured handler reads acknowledged file bytes, staged Git status and newly attached task bytes. |
| Documents retain canonical storage and path/error contracts | `observation_mixed::mixed_plan_document_keeps_canonical_store_and_error_parity`; the 22-verb fixture's `task.doc`/`task.stage_doc` response checks | Store-backed contents, unavailable/escaping paths, stage prefix containment and successful nonempty document bodies. |
| Review versions remain bound to selected work | `observation_mixed::mixed_review_delayed_work_keeps_its_selected_version_and_next_read_is_fresh`; existing `reviews::review_rpc_reads_whole_saved_files_and_refuses_caller_supplied_locations` | A prepared version-1 get/diff stays selected across version 2; the next read is fresh. Saved immutable blobs ignore unsaved checkout changes. |
| Task/thread attachment fences, ranges and owner generations | `observation_mixed::{mixed_task_attachment_keeps_path_fences_ranges_and_write_visibility,mixed_thread_attachment_keeps_homes_ranges_and_reset_removal_fences}`; existing `conversations::attachments::thread_attachment_pages_are_bounded_and_cover_the_last_byte` | Store containment, symlink refusal, exact byte ranges, reset/removal generation fences and complete final-byte paging. |
| Archive and Git-init readers retain observations | `observation_mixed::{mixed_archive_preserves_finished_registry_rows,mixed_archive_uses_published_branch_without_reopening_git}`; successful `workspace.git_init_options` in every timing scenario | Finished legacy registry rows and published branch identity; both real workspace/source Git-init probes complete successfully. |
| Existing external errors/deadlines remain | `observation_reads::github_tests::{independent_github_handler_preserves_refusal_while_the_app_is_held,independent_github_handler_preserves_the_external_command_deadline}`; existing Git process deadline tests | Injected local CLI refusal with app held; existing 20-second GitHub deadline; bounded Git child termination. |
| Deletion and publication generations cannot authorize stale work | `observation_reads::tests::project_deletion_keeps_allowed_observations_and_refuses_task_reads`; `read_view::tests::stale_observation_context_rejects_the_entire_candidate_delta` | Existing deletion exceptions and whole-candidate generation rejection. |


Additional scope regressions cover successful deletion after a captured read,
failed run path/base/stage candidates followed by unrelated publication,
completed retained-stage lineage selection and resolver purity:
`scope_workspace_delete_retires_captured_workspace_and_run_addresses`,
`scope_failed_run_path_base_and_stage_candidate_stays_out_of_later_reads`,
`scope_stage_reads_choose_completed_retained_lineage_and_its_original_repository`
and `scope_reads_never_adopt_legacy_records_or_schedule_external_scans`.
Their setup explicitly pins the publication producers so reads cannot hide
maintenance or silently choose a later incomplete lineage.

Additional mixed-reader coverage uses the already initialized service across
real reset/removal writes (`mixed_thread_attachment_service_observes_real_reset_and_agent_removal`),
retains committed document paths after a failed save and unrelated publication
(`mixed_plan_documents_retain_committed_paths_after_failed_save_and_unrelated_publication`),
checks reclaim reservation/release publication
(`mixed_review_destinations_publish_reclaim_reservation_and_release`) and
bounds archive capture to one visit per retained row at 512 rows
(`mixed_archive_capture_visits_retained_records_once`).
`store::read_pool::tests::review_metadata_uses_one_readonly_snapshot_without_the_writer_connection`
pins review metadata to one query-only WAL transaction while the writer
connection is held; the mixed external gate tests separately verify that the
connection is returned before I/O.

`mixed_capture_indexes_repeated_lineages_and_workspaces_once_with_legacy_home_parity`
counts actual mixed-context capture at four plans with three retained
lineages each and three projects: each run is indexed once, with two
workspace listing traversals for the archive and review projections.
Attachment answers match the legacy resolver, including equal-time lineage
ties and terminal filtering. This is separate producer work-count evidence;
the small timing fixture does not establish large-project publication latency,
startup cost, memory growth or optimal worker counts.

The baseline archive ran the three existing-API isolation regressions before
production migration: `file_and_git_observations_answer_while_the_app_mutex_is_held`,
`observation_reads_bypass_the_held_write_worker` and
`io_observations_are_independent_of_application_mutex` each exited **101** on
its held-resource timeout. The symlink/byte-bound control exited **0**.
`/tmp/task694-before/red-exits.json` and the `*.final.log` files preserve these
outcomes against the final matching fixture/binary.

The integrated reclaim-reservation regression also failed before its producer
fix: `mixed_review_destinations_publish_reclaim_reservation_and_release`
exited **101**, finding a missing reserved-destination error. The initial
integrated binary/source provenance and `/tmp/task694-gates/reservation-red.log`
retain that evidence; the later producer fix publishes both reservation and
release before subsequent reads.



| Startup/timing requirement | Coverage | Evidence |
| --- | --- | --- |
| One queued observation wakes one coordinator | `carrier::sql::queue_wakeup_tests::one_queued_read_wakes_one_idle_execution_coordinator` | Deterministic wake count, before/after regression |
| Internal handler timer excludes reply sealing consistently | `carrier::observation_tests::observation_handler_timing_ends_before_sealing_its_reply` | Endpoint regression independent of full client timer |
| I/O grows only with concurrent work and remains bounded | `carrier::observation_tests::io_threads_grow_only_for_concurrent_work_and_reuse_warm_workers`; `carrier::io::tests::serial_submissions_reuse_one_worker_after_actual_execution_completion` | First request, quiescent reuse, eight occupied workers and queued ninth; real client counters remain 1–2 |
| SQL 8 is ready before intake and shared by handler clones | `carrier::observation_tests::eager_sql_workers_are_ready_before_intake_and_shared_by_handler_clones` | Preaccept 0→8, idempotence, shared pool; no warmup query |
| Every read connection has schema state before first application-table preparation | `store::read_pool::schema_tests::{raw_reader_negative_control_allocates_schema_on_first_table_prepare,all_pooled_readers_have_loaded_schemas_before_first_table_prepare}` | Eight distinct readers plus raw negative control; startup durations remain visible |
| SQL metadata capacity remains eight and connections release before I/O | `carrier::observation_tests::{a_cold_mixed_observation_initializes_eight_sql_workers,mixed_sql_preparation_shares_eight_permits_with_ordinary_reads}` and existing external-gate regressions | No SQL-demand mode remains; ordinary/mixed capacity and release fences preserved |
| Final Store/read-pool teardown checkpoints WAL and preserves file-only backups | `store::tests::operations::{a_closed_store_keeps_saved_runs_in_the_database_file_alone,a_backup_is_a_whole_store_and_never_silently_replaces_one}`; `store::read_pool::tests::{a_cloned_read_pool_keeps_the_writer_until_its_readers_close,a_borrowed_reader_keeps_the_writer_until_the_final_pool_owner_closes}` | All four exact regressions exit 101 before the lifetime fix and 0 after; cloned pool and outstanding borrower both retain the writer until readers close |

## Validation and reproduction evidence

The runner and analysis sources are included in the evidence export. Reproduce the final measurement with frozen baseline/final binaries and the exact sidecars recorded in the manifest; use a new output directory and retain every run. The default final fixture has fixed eager SQL startup; there is no startup-policy override.

```sh
nice -n 10 python /tmp/task694-bench-runner.py \
  --baseline-binary /tmp/task694-latency/baseline-supplemental/test-binary \
  --candidate-binary /tmp/task694-latency/selected-eager-final/build/test-binary \
  --baseline-provenance /tmp/task694-latency/baseline-supplemental/provenance.json \
  --candidate-provenance /tmp/task694-latency/selected-eager-final/build/provenance.json \
  --candidate-provenance /tmp/task694-latency/selected-eager-final/selected-policy.json \
  --output-root /tmp/task694-reproduction \
  --pairs 4 --samples 128 --warmups 16 --cold 16 --trace 0
nice -n 10 python /tmp/task694-final-comparison.py \
  /tmp/task694-reproduction --output /tmp/task694-reproduction-analysis
nice -n 10 python /tmp/task694-acceptance-resources.py \
  /tmp/task694-reproduction --output /tmp/task694-reproduction-analysis/resources.json
nice -n 10 python /tmp/task694-startup-summary.py /tmp/task694-reproduction-analysis
```

Baseline reconstruction starts from `git archive 54e8e1d791c4f93d596a4aafb06487207084c27e` and applies the exact declared test-only overlay, whose SHA-256 is `4a4b7ac944a732e647c4287242151f300accab580083a18ff03128a066bbe5d4`. The measured selected source uses aa26175 unchanged. Both build with `nice -n 10 cargo test --lib --no-run --message-format=json`; provenance records toolchain, flags, source paths, fixture hashes and the copied compiler artifact. The runner invokes the ignored `paired_observation_benchmark` directly; these manual performance runs are separate from the unfiltered correctness gate. Target-first reproduction adds `--first-method tasks.attachment` or `tasks.review.diff` and `--scenario isolated`. Historical P control requires its recorded old eager override and is intentionally not substituted for the selected binary.

The first full gate on measured aa26175 exposed a **real backup correctness failure**: the existing whole-store backup test restored zero saved rows instead of one. Schema-initialized read connections participate in WAL, and Store teardown closed its writable connection before its read pool; the final read-only connection could not perform the final checkpoint. The narrow c4e3e8ee2 fix retains the same private writer Arc until every pooled reader closes. It adds no writer-mutex acquisition, query or scheduling step to observation reads. Cloned pool and outstanding-borrower cases are covered explicitly.

All four exact shutdown/backup regressions exited **101 before → 0 after** that fix. `/tmp/task694-latency/writer-lifetime-{red,fixed}/regressions.json` and their logs retain each result. The first full gate is also retained at `/tmp/task694-latency/gates/runs/20261020T183002.706544Z`: 5,178 unit tests passed, one failed, 12 were ignored; its other 16 checks passed. It is not the final green gate.

Final-source focused checks passed 10 read-pool, 108 carrier and four observation tests. Its three correctness smokes passed 220 successful observations: 132 across all three scenarios, plus 44 each with attachment and review-diff first. An independent parse verified fixed eager8/no switch, schema count eight, SQL 0→8 before intake, every observation count delta one, exact retained phase sums and zero app wait/hold. Payload validity is asserted by the successful fixture. `/tmp/task694-latency/writer-lifetime-fixed/smoke-validation.json` records those checks. These smokes verify final-source behavior; they are **not a new acceptance latency capture**.

The corrected final gate ran against unchanged source **c4e3e8ee2265cc7c9f8856ccaf521faa4db5dc90**. All **17 checks exited 0**, aggregate exit 0. The unfiltered Cargo run passed **5,408 tests with 16 ignored across 33 target summaries**, including **5,182 library tests with 12 ignored**. Ignored performance diagnostics remain separate from those correctness counts. Both Clippy toolchains checked all targets with warnings denied. Exact commands, environment, timings and logs are in `/tmp/task694-latency/gates/runs/20261020T184239.010527Z/summary.json` and its per-check exit files.

| Gate | Exit |
| --- | --- |
| `toolchain-cargo-local` | 0 |
| `toolchain-cargo-stable` | 0 |
| `toolchain-node` | 0 |
| `toolchain-npm` | 0 |
| `cargo-test` | 0 |
| `clippy-local` | 0 |
| `clippy-stable` | 0 |
| `fmt` | 0 |
| `vendor` | 0 |
| `pi-install` | 0 |
| `pi-test` | 0 |
| `pi-typecheck` | 0 |
| `semgrep` | 0 |
| `gitleaks` | 0 |
| `diff-check-committed` | 0 |
| `diff-check-worktree` | 0 |
| `diff-check-index` | 0 |


The timing tables measure aa26175, while the final correctness gates exercise the later lifetime-corrected source identified above. There is no new timing cohort for that final shutdown-only change, so the tables are not relabeled as measurements of it. A later documentation-only commit does not alter either source. Full JSON/CSV output retains all paired values and recorded setup, workers and machine context. The final evidence export includes source/binary/overlay identities, valid RED and GREEN records, the excluded stale artifact, historical failed cohorts and reconstruction instructions. Large raw captures remain indexed by exact path/hash; no claim is based on a filtered subset.

The attachable plain-text evidence export is under `/tmp/task694-evidence-final/`: `task694-results-01.txt` (reports, exact CSV/JSON and analysis sources), `task694-capture-controls-01.txt` (run/configuration/setup/worker records), `task694-provenance-01.txt` (source, overlay and binary/build identity) and `task694-gates-history-01.txt` (gates, RED/GREEN and retained investigation inventory). `task694-evidence-index.log` identifies exact inputs and hashes; `task694-evidence-readme.txt` describes extraction and reproduction. Parts remain below the attachment size cap. The export is evidence of the disclosed result, not a statement of latency equivalence.

Primary retained files:

- `/tmp/task694-latency/selected-final-full22/manifest.json` and its eight run directories preserve every raw request, configuration, setup, FrameClock and resource record.
- `/tmp/task694-latency/selected-final-full22-analysis/{measurements.csv,summary.json,process-first-and-workers.json,startup-next-to-client.json,resources.json,comparison-semantics.json,tables.md}` preserve all final paired values and validation output.
- `/tmp/task694-latency/selected-eager-results/short-residual-table.{md,json}` retain the distinctly identified target-first a74 control.
- `/tmp/task694-latency/direct-attempt-{attachment,review-diff}` and their analysis directories preserve every baseline/pre-direct/dropped-attempt run.
- `/tmp/task694-latency/selected-eager-final/{build/provenance.json,selected-policy.json}` record the measured selected source and final policy; the gate run directory records the independently executed checks.
- `/tmp/task694-step4-evidence.txt` and the linked historical report retain the original eight-sample evidence and corrected endpoint interpretation.

Raw JSONL and resource logs remain local with exact paths and hashes in the index; historical manifests retain prior cohort identities and failures. No selected low-load subset replaces the original captures.

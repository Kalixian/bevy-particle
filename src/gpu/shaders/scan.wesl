// Prefix sums, for workgroups of 256.

const WG: u32 = 256u;

var<workgroup> wg_scan: array<u32, WG>;

// Exclusive prefix sum across the workgroup (Hillis-Steele). Returns (exclusive, total).
fn scan_exclusive(lid: u32, v: u32) -> vec2<u32> {
    wg_scan[lid] = v;
    workgroupBarrier();
    for (var offset = 1u; offset < WG; offset <<= 1u) {
        var t = 0u;
        if lid >= offset {
            t = wg_scan[lid - offset];
        }
        workgroupBarrier();
        wg_scan[lid] += t;
        workgroupBarrier();
    }
    return vec2(wg_scan[lid] - v, wg_scan[WG - 1u]);
}

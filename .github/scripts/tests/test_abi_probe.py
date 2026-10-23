"""The project half of the release guard: .github/hyper.json and .github/scripts/abi_probe.c.

The generic half (trailers, payload paths, staging, provenance) lives in the Forge
(SkunkWerkx/.github, release_guard.py) and reads this repository's names from hyper.json.
These tests hold the two files the Forge is pointed at: the export rule in hyper.json must
read the C ABI exports.rs declares (rust/check-core.sh compares the same list with the built
library), and the probe must compile against the shipped header and tell a right payload from
a wrong one when linked the way the Forge links it."""
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
CONFIG = ROOT / '.github/hyper.json'
BUNDLE_INCLUDE = ROOT / 'swift/HyperTabularCore.artifactbundle/include'
EXPORTS = {
    'hypertabular_version',
    'hypertabular_delimited_state_size', 'hypertabular_delimited_init', 'hypertabular_delimited_header',
    'hypertabular_delimited_fill', 'hypertabular_delimited_unescape',
    'hypertabular_workbook_state_size', 'hypertabular_workbook_open', 'hypertabular_workbook_sheets',
    'hypertabular_workbook_strings', 'hypertabular_workbook_styles', 'hypertabular_workbook_sheet',
    'hypertabular_workbook_header', 'hypertabular_workbook_fill',
}


class HyperConfigTests(unittest.TestCase):
    def setUp(self):
        self.cfg = json.loads(CONFIG.read_text())

    def test_names_and_files_the_forge_is_pointed_at(self):
        self.assertEqual((self.cfg['crate'], self.cfg['project']), ('hypertabular', 'HyperTabular'))
        self.assertNotIn('cdylib_smoke', self.cfg, 'HyperTabular has no cdylib smoke script')
        self.assertTrue((ROOT / self.cfg.get('abi_probe', '.github/scripts/abi_probe.c')).is_file())
        self.assertTrue((BUNDLE_INCLUDE / 'hypertabular.h').is_file())

    def test_export_rule_reads_the_fourteen_exports_and_not_the_canary(self):
        exports = self.cfg['exports']
        names = set()
        for source in exports['sources']:
            text = (ROOT / source).read_text()
            for pattern in exports['patterns']:
                self.assertIsInstance(pattern, str, 'HyperTabular declares its exports one per line')
                names |= set(re.findall(pattern, text))
        self.assertIn('hypertabular_canary', names, 'the no-panic canary is declared like an export')
        names -= set(exports.get('exclude', ()))
        self.assertEqual(names, EXPORTS)
        prefixes = tuple(exports['prefixes'])
        self.assertEqual({n for n in names if not n.startswith(prefixes)}, set(),
                         'every export must carry a prefix the symbol check recognises')


class AbiProbeTests(unittest.TestCase):
    """abi_probe.c compiled and run the way the Forge's check_runtime does it."""
    def setUp(self):
        if not shutil.which('cc'):
            self.skipTest('needs a C compiler')
        self.probe = ROOT / '.github/scripts/abi_probe.c'

    def run_probe(self, payload, version):
        packed = sum(int(n) << shift for n, shift in zip(version.split('.'), (16, 8, 0)))
        with tempfile.TemporaryDirectory() as tmp:
            exe = Path(tmp) / 'abi'
            subprocess.run(['cc', f'-DEXPECTED_VERSION={packed}', '-I', str(BUNDLE_INCLUDE), str(self.probe),
                            str(payload.resolve()), '-ldl', '-lpthread', '-lm', '-o', str(exe)], check=True)
            subprocess.run([str(exe)], check=True)

    def test_version_layout_and_result_contract(self):
        # Shared and static fixtures with the header's prototypes, written to satisfy exactly
        # what the probe checks: a wrong version, a fill that reports the wrong row count, or
        # one that accepts a format whose size field is wrong must each fail; the right one passes.
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            for version, rows, contract, accepted in [('0x000900', 2, -1, True), ('0x000800', 2, -1, False),
                                                       ('0x000900', 1, -1, False), ('0x000900', 2, 0, False)]:
                c = tmp / 'fixture.c'
                c.write_text(f'''#include <stdint.h>
#include <string.h>
#include "hypertabular.h"
uint32_t hypertabular_version(void) {{return {version};}}
uintptr_t hypertabular_delimited_state_size(void) {{return sizeof(hypertabular_delimited_state);}}
int32_t hypertabular_delimited_init(hypertabular_delimited_state *s, const hypertabular_dialect *d) {{(void)s;(void)d; return 0;}}
int32_t hypertabular_delimited_fill(hypertabular_delimited_state *s, const uint8_t *in, uintptr_t len, uint32_t last,
    const hypertabular_column_spec *spec, const hypertabular_column_buffer *columns, uintptr_t count, uintptr_t cap,
    const hypertabular_buffers *b, hypertabular_filled *filled) {{
    (void)s;(void)len;(void)last;(void)count;(void)b;(void)cap;
    if (spec->format.size != 0 && spec->format.size != sizeof(spec->format)) return {contract};
    if (spec->door == 11) {{ *(double *)columns->values = 1234.5; columns->verdicts[0].reason = 0; filled->rows = 1; return 0; }}
    int32_t *v = columns->values; v[0] = in[0] == '1' ? 123 : 0; columns->verdicts[0].reason = 0; columns->verdicts[1].reason = 2;
    filled->rows = {rows}; return 0; }}
''')
                obj, shared, archive = tmp / 'f.o', tmp / 'f.so', tmp / 'f.a'
                subprocess.run(['cc', '-fPIC', '-I', str(BUNDLE_INCLUDE), '-c', str(c), '-o', str(obj)], check=True)
                subprocess.run(['cc', '-shared', str(obj), '-o', str(shared)], check=True)
                archive.unlink(missing_ok=True)
                subprocess.run(['ar', 'rcs', str(archive), str(obj)], check=True)
                for payload in (shared, archive):
                    with self.subTest(version=version, rows=rows, contract=contract, payload=payload.suffix):
                        if accepted:
                            self.run_probe(payload, '0.9.0')
                        else:
                            with self.assertRaises(subprocess.CalledProcessError):
                                self.run_probe(payload, '0.9.0')

    def test_probe_matches_the_shipped_header(self):
        # A declaration the probe uses that the header no longer has, or a changed field, is a
        # compile error here before the Forge meets it at staging time.
        subprocess.run(['cc', '-fsyntax-only', '-Werror', '-DEXPECTED_VERSION=0', '-I', str(BUNDLE_INCLUDE),
                        str(self.probe)], check=True)
        with tempfile.TemporaryDirectory() as tmp:
            broken = Path(tmp) / 'broken.c'
            broken.write_text(self.probe.read_text().replace('hypertabular_delimited_fill', 'hypertabular_delimited_filling'))
            with self.assertRaises(subprocess.CalledProcessError):
                subprocess.run(['cc', '-fsyntax-only', '-Werror', '-DEXPECTED_VERSION=0', '-I', str(BUNDLE_INCLUDE),
                                str(broken)], check=True, stderr=subprocess.DEVNULL)


if __name__ == '__main__':
    unittest.main()

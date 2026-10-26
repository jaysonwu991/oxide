import contextlib
import hashlib
import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from inspect_pptx import A, P, R, inspect, main


REL = "http://schemas.openxmlformats.org/package/2006/relationships"


def rels(*items):
    return f'<Relationships xmlns="{REL}">' + "".join(
        f'<Relationship Id="{key}" Type="{R}/{kind}" Target="{target}"{extra}/>'
        for key, kind, target, extra in items
    ) + '</Relationships>'


def fixture(path, broken=False):
    parts = {
        'ppt/presentation.xml': f'<p:presentation xmlns:p="{P}" xmlns:r="{R}"><p:sldIdLst><p:sldId id="300" r:id="second"/><p:sldId id="301" r:id="first"/></p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/></p:presentation>',
        'ppt/_rels/presentation.xml.rels': rels(('first', 'slide', 'slides/slide1.xml', ''), ('second', 'slide', 'slides/slide9.xml', '')),
        'ppt/slides/slide1.xml': f'<p:sld xmlns:p="{P}" xmlns:a="{A}"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Last slide</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>',
        'ppt/slides/slide9.xml': f'<p:sld xmlns:p="{P}" xmlns:a="{A}" show="0"><p:cSld><p:spTree><p:pic/><p:cxnSp/><p:sp><p:spPr><a:custGeom/></p:spPr><p:txBody><a:p><a:r><a:t>First </a:t></a:r><a:r><a:t>slide</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>',
        'ppt/slides/_rels/slide9.xml.rels': rels(('layout', 'slideLayout', '../slideLayouts/layout1.xml', ''), ('notes', 'notesSlide', '../notesSlides/note1.xml', ''), ('link', 'hyperlink', 'https://example.com/not-a-part', ' TargetMode="External"')),
        'ppt/slideLayouts/layout1.xml': f'<p:sldLayout xmlns:p="{P}"><p:cSld><p:spTree><p:pic/></p:spTree></p:cSld></p:sldLayout>',
        'ppt/slideLayouts/_rels/layout1.xml.rels': rels(('master', 'slideMaster', '../slideMasters/master1.xml', '')),
        'ppt/slideMasters/master1.xml': f'<p:sldMaster xmlns:p="{P}"><p:cSld><p:spTree><p:pic/></p:spTree></p:cSld></p:sldMaster>',
        'ppt/notesSlides/note1.xml': f'<p:notes xmlns:p="{P}" xmlns:a="{A}"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Source and speaker detail</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>',
    }
    if broken:
        del parts['ppt/slideMasters/master1.xml']
    with zipfile.ZipFile(path, 'w') as archive:
        for name, body in parts.items():
            archive.writestr(name, body)


class InspectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name) / 'deck.pptx'
        fixture(self.source)

    def test_presentation_order_notes_and_inherited_graphics(self):
        before = self.source.read_bytes()
        report = inspect(self.source, include_notes=True)
        self.assertEqual(report['slide_count'], 2)
        self.assertEqual(report['slide_size']['inches'], [13.3333, 7.5])
        self.assertEqual([s['paragraphs'][0] for s in report['slides']], ['First slide', 'Last slide'])
        first = report['slides'][0]
        self.assertTrue(first['hidden'])
        self.assertEqual(first['notes'], ['Source and speaker detail'])
        self.assertEqual(first['graphics']['custom_shapes'], 1)
        self.assertEqual(first['graphics']['connectors'], 1)
        self.assertEqual([p['graphics']['images'] for p in first['inherited']], [1, 1])
        self.assertEqual(report['package_errors'], [])
        self.assertEqual(self.source.read_bytes(), before)

    def test_slide_selection_and_invalid_range(self):
        self.assertEqual([s['number'] for s in inspect(self.source, '2')['slides']], [2])
        self.assertNotIn('notes', inspect(self.source)['slides'][0])
        for selection in ('0', '3', '2-1', '1-2-3'):
            with self.subTest(selection=selection), self.assertRaises(ValueError):
                inspect(self.source, selection)

    def test_missing_internal_part_is_a_failure_with_a_saved_report(self):
        fixture(self.source, broken=True)
        output = Path(self.temp.name) / 'report.json'
        with patch('sys.argv', ['inspect_pptx.py', str(self.source), '--output', str(output)]), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main(), 2)
        self.assertTrue(any('master1.xml' in e for e in json.loads(output.read_text())['package_errors']))

    def test_invalid_zip_fails_without_traceback(self):
        self.source.write_bytes(b'not a zip')
        error = io.StringIO()
        with patch('sys.argv', ['inspect_pptx.py', str(self.source)]), contextlib.redirect_stderr(error):
            self.assertEqual(main(), 2)
        self.assertIn('PPTX inspection failed:', error.getvalue())

    def test_report_cannot_replace_the_input_or_an_alias(self):
        digest = hashlib.sha256(self.source.read_bytes()).digest()
        alias = Path(self.temp.name) / 'alias.pptx'
        alias.hardlink_to(self.source)
        for output in (self.source, alias):
            with self.subTest(output=output), patch('sys.argv', ['inspect_pptx.py', str(self.source), '--output', str(output)]), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(main(), 2)
        self.assertEqual(hashlib.sha256(self.source.read_bytes()).digest(), digest)


if __name__ == '__main__':
    unittest.main()

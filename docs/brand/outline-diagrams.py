"""Export editable SVG labels as paths so GitHub needs no font downloads.

Install fonttools==4.63.0 outside the product environment, then run this file.
The editable inputs are diagram-sources/*.svg; outputs are docs/assets/*.svg.
"""
from pathlib import Path
import xml.etree.ElementTree as ET

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont

NS = 'http://www.w3.org/2000/svg'
ET.register_namespace('', NS)
ROOT = Path(__file__).resolve().parent
FONTS = {'hand': 'reenie-beanie.ttf', 'body': 'spline-sans.ttf'}


def outline_label(element, fonts):
    """Keep the accessible label and positioning while outlining its glyphs."""
    text = element.text or ''
    font = fonts[element.attrib.pop('data-font')]
    glyphs = font.getGlyphSet()
    cmap = font.getBestCmap()
    scale = float(element.attrib.pop('font-size')) / font['head'].unitsPerEm
    names = [cmap[ord(char)] for char in text]
    width = sum(glyphs[name].width for name in names) * scale
    x = float(element.attrib.pop('x'))
    y = float(element.attrib.pop('y'))
    anchor = element.attrib.pop('text-anchor', 'start')
    if anchor == 'middle':
        x -= width / 2
    elif anchor == 'end':
        x -= width
    element.tag = f'{{{NS}}}g'
    element.set('aria-label', text)
    element.set('role', 'img')
    element.text = None
    offset = 0
    for name in names:
        pen = SVGPathPen(glyphs)
        glyphs[name].draw(pen)
        commands = pen.getCommands()
        if commands:
            ET.SubElement(element, f'{{{NS}}}path', {
                'd': commands,
                'transform': f'translate({x + offset * scale:.3f} {y}) scale({scale:.6f} {-scale:.6f})',
            })
        offset += glyphs[name].width
    return width


def export_diagrams():
    fonts = {key: TTFont(ROOT / 'assets' / name) for key, name in FONTS.items()}
    destination = ROOT.parent / 'assets'
    destination.mkdir(exist_ok=True)
    for source in sorted((ROOT / 'diagram-sources').glob('*.svg')):
        tree = ET.parse(source)
        labels = list(tree.getroot().iter(f'{{{NS}}}text'))
        for element in labels:
            outline_label(element, fonts)
        target = destination / source.name
        ET.indent(tree, space='  ')
        tree.write(target, encoding='unicode', xml_declaration=False)
        with target.open('a') as output:
            output.write('\n')
        print(f'{target.name}: {len(labels)} outlined labels')


if __name__ == '__main__':
    export_diagrams()

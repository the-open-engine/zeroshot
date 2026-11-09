# Plugin icon

`zeroshot-seal.png` renders the existing square `docs/brand/zeroshot-seal.svg` at
512 × 512 pixels. It preserves that mark and its transparency. The plugin generator
copies this checked-in PNG; generation and installation need no graphics tooling.

To refresh it after an intentional change to the source vector, use CairoSVG's
`svg2png` with `output_width=512` and `output_height=512`, then regenerate both
packages. Each package includes the repository's MIT license.

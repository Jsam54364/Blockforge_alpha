# Textures

All block textures here are original pixel art, painted by `tools/paint_textures.py`
(32 x 32, seamless; short hue-shifted colour ramps, shapes lit from the top left, no lone
pixels). Grass tops and leaves are greyscale and tinted by biome in the game; a grass side is
dirt plus `<name>_overlay.png`, the greyscale fringe. Edit the PNGs directly, or change the
painter and run it again: `python3 tools/paint_textures.py` (needs numpy and Pillow).

# The character

`hyrig.bin` and `hyrig.png` are the **HyRig** character with its 51 animation clips, converted
by `tools/convert_hyrig.py` from the HyRig controller kit's `hyrig.glb`.

The HyRig model is by **Powerbyte7** (https://sketchfab.com/Powerbyte7), "HyRig
Blender/CraftStudio port", edited in the part editor, and is licensed
**CC BY-NC 4.0** (http://creativecommons.org/licenses/by-nc/4.0/): credit is required and
**commercial use is not allowed**. That licence covers `hyrig.bin` and `hyrig.png`; show the
credit somewhere in anything released with them.

`hyrig_controller.json` (the state machine's numbers) is separate from the model and works
with any character that has the same bone and clip names. `app/src/hyrig` is a Rust port of
the kit's controller; `app/tests/data/hyrig_reference_trace.json` is the kit's recorded
reference run, which the port is checked against.

To use a newer kit: `python3 tools/convert_hyrig.py path/to/hyrig_kit`, copy its
`tests/reference_trace.json` over the one in `app/tests/data`, rebuild and run the tests.

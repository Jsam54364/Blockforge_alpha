# Sky

`sky.png` and `glow.txt` are baked by `tools/bake_sky.py` from **Rustig Koppie Pure Sky**
(2K HDRI) by Poly Haven, https://polyhaven.com/a/rustig_koppie_puresky, released as CC0
(public domain). The sun has been taken out of it (the game draws its own, which moves), and
only the half above the horizon is kept.

To use another sky: download an HDRI (equirectangular `.hdr`, 2K is plenty), run
`python3 tools/bake_sky.py that.hdr`, rebuild. Clear or lightly clouded skies work best:
clouds stay where they are and stay lit from where the photo's sun was.

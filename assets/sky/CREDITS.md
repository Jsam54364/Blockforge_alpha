# Sky

`sky.png` and `glow.txt` are baked by `tools/bake_sky.py` from **Rustig Koppie Pure Sky**
(2K HDRI) by Poly Haven, https://polyhaven.com/a/rustig_koppie_puresky, released as CC0
(public domain). The sun has been taken out of it (the game draws its own, which moves), and
only the half above the horizon is kept.

`night.png` and `night.txt` are baked by `tools/bake_sky.py --night` from **Night Sky HDRI
003** (2K EXR) by ambientCG, https://ambientcg.com/view?id=NightSkyHDRI003, released as CC0
(public domain). Its smooth background has been taken out (the game draws its own by
elevation, in the photo's colours, from `night.txt`); the stars, Milky Way and moon are kept.

To use another sky: download an HDRI (equirectangular `.hdr` or `.exr`, 2K is plenty), run
`python3 tools/bake_sky.py that.hdr` for the day or `python3 tools/bake_sky.py --night that.exr`
for the night, rebuild. Clear or lightly clouded day skies work best: clouds stay where they
are and stay lit from where the photo's sun was. A night sky should have its moon in it
(the brightest spot): the game turns the photo so that moon is the game's moon.

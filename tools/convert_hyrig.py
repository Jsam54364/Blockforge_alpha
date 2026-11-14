#!/usr/bin/env python3
"""Converts the HyRig kit's character (hyrig.glb) into the game's own small format.

    python3 tools/convert_hyrig.py path/to/hyrig_kit

The model is a tree of rigid parts (no skinning): every node has its own little mesh and the
animations turn and move the nodes. That is all the game needs, so the converter writes just
that, and the game needs no glTF reader:

    assets/hyrig/hyrig.bin             nodes, their meshes, and all animation clips (below)
    assets/hyrig/hyrig.png             the texture
    assets/hyrig/hyrig_controller.json the state machine's numbers, copied as they are

hyrig.bin, little-endian:
    "HYRIG1\\0\\0"
    u32 node count, then per node:
        u8 name length, name; i32 parent (-1 = none); f32 x 3 translation; f32 x 4 rotation
        (x y z w); f32 x 3 scale; u32 vertex count; per vertex f32 x 8: position, normal, uv
        (triangles, three vertices each, in the node's own space)
    u32 clip count, then per clip:
        u8 name length, name; f32 duration; u32 channel count; per channel:
            u16 node; u8 path (0 = translation, 3 floats per key; 1 = rotation, 4 per key);
            u32 key count; f32 x count times; f32 x (count * 3 or 4) values
    Keys are interpolated linearly (rotations: normalised, shortest way round).

Needs only Python's standard library.
"""
import json
import os
import shutil
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'assets', 'hyrig')
COMPONENTS = {'SCALAR': 1, 'VEC2': 2, 'VEC3': 3, 'VEC4': 4}
FORMATS = {5126: 'f', 5123: 'H', 5125: 'I', 5121: 'B'}


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    kit = sys.argv[1]
    raw = open(os.path.join(kit, 'hyrig.glb'), 'rb').read()
    json_len = struct.unpack('<I', raw[12:16])[0]
    g = json.loads(raw[20:20 + json_len])
    blob = raw[20 + json_len + 8:]

    def accessor(i):
        a = g['accessors'][i]
        view = g['bufferViews'][a['bufferView']]
        n = COMPONENTS[a['type']]
        fmt = FORMATS[a['componentType']]
        size = struct.calcsize(fmt) * n
        stride = view.get('byteStride', size)
        start = view.get('byteOffset', 0) + a.get('byteOffset', 0)
        rows = [struct.unpack_from('<' + fmt * n, blob, start + k * stride) for k in range(a['count'])]
        return rows

    nodes = g['nodes']
    parent = {c: i for i, n in enumerate(nodes) for c in n.get('children', [])}
    out = bytearray(b'HYRIG1\0\0')
    out += struct.pack('<I', len(nodes))
    vertices = 0
    for i, n in enumerate(nodes):
        name = n['name'].encode()
        out += struct.pack('<B', len(name)) + name
        out += struct.pack('<i', parent.get(i, -1))
        out += struct.pack('<3f', *n.get('translation', [0, 0, 0]))
        out += struct.pack('<4f', *n.get('rotation', [0, 0, 0, 1]))
        out += struct.pack('<3f', *n.get('scale', [1, 1, 1]))
        tris = []
        if 'mesh' in n:
            for prim in g['meshes'][n['mesh']]['primitives']:
                pos = accessor(prim['attributes']['POSITION'])
                nor = accessor(prim['attributes']['NORMAL'])
                uv = accessor(prim['attributes']['TEXCOORD_0'])
                idx = [r[0] for r in accessor(prim['indices'])] if 'indices' in prim else range(len(pos))
                tris += [pos[k] + nor[k] + uv[k] for k in idx]
        assert len(tris) % 3 == 0
        out += struct.pack('<I', len(tris))
        for v in tris:
            out += struct.pack('<8f', *v)
        vertices += len(tris)

    out += struct.pack('<I', len(g['animations']))
    keys = 0
    for anim in g['animations']:
        name = anim['name'].encode()
        channels = [c for c in anim['channels'] if c['target']['path'] in ('translation', 'rotation')]
        # (the blink clip only scales the eyelids; the game blinks by its own rules, so it has no channels here)
        duration = max(accessor(anim['samplers'][c['sampler']]['input'])[-1][0] for c in anim['channels'])
        out += struct.pack('<B', len(name)) + name + struct.pack('<fI', duration, len(channels))
        for c in channels:
            s = anim['samplers'][c['sampler']]
            assert s.get('interpolation', 'LINEAR') == 'LINEAR'
            times = [r[0] for r in accessor(s['input'])]
            values = accessor(s['output'])
            path = 0 if c['target']['path'] == 'translation' else 1
            out += struct.pack('<HBI', c['target']['node'], path, len(times))
            out += struct.pack(f'<{len(times)}f', *times)
            for v in values:
                out += struct.pack(f'<{len(v)}f', *v)
            keys += len(times)

    os.makedirs(OUT, exist_ok=True)
    open(os.path.join(OUT, 'hyrig.bin'), 'wb').write(out)
    image = g['bufferViews'][g['images'][0]['bufferView']]
    open(os.path.join(OUT, 'hyrig.png'), 'wb').write(blob[image.get('byteOffset', 0):][:image['byteLength']])
    shutil.copy(os.path.join(kit, 'hyrig_controller.json'), os.path.join(OUT, 'hyrig_controller.json'))
    print(f'{len(nodes)} nodes, {vertices} vertices, {len(g["animations"])} clips, {keys} keys: '
          f'hyrig.bin is {len(out) / 1e3:.0f} kB')


if __name__ == '__main__':
    main()

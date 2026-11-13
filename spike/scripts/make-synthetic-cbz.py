#!/usr/bin/env python3
"""Gera spike/fixtures/synthetic-600mb.cbz: paginas do Little Nemo repetidas 2x, nomes distintos
SEM zero-padding (page1.jpeg ... page842.jpeg) para exercitar ordenacao natural; modo stored; mantem __MACOSX."""
import zipfile, sys, os
src = 'fixtures/little-nemo-1905-1914.cbz'; dst = 'fixtures/synthetic-600mb.cbz'
zin = zipfile.ZipFile(src)
pages = sorted((i for i in zin.infolist() if not i.filename.startswith('__MACOSX') and i.filename.endswith('.jpeg')), key=lambda i: i.filename)
n = 0
with zipfile.ZipFile(dst, 'w', zipfile.ZIP_STORED) as zo:
    for rep in range(2):
        for i in pages:
            n += 1
            zo.writestr(f'page{n}.jpeg', zin.read(i))
            zo.writestr(f'__MACOSX/._page{n}.jpeg', b'x'*226)
print(n, 'paginas;', os.path.getsize(dst), 'bytes')

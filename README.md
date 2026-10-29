# KindleUpdater

A gépen lévő e-könyv könyvtárakat tükrözi a Kindle-re. A gép a mester példány:
ami a gépen megvan, az felkerül, ami ott nincs meg, az a Kindle-ről is törlődik
(nagyjából `rsync -av --delete`).

| Gép                        | Kindle                |
| -------------------------- | --------------------- |
| `~/Könyvek/Könyvtár/Magyar` | `documents/hungarian` |
| `~/Könyvek/Könyvtár/Angol`  | `documents/english`   |

## Használat

```sh
kindle-updater            # terv, megerősítés (i/N), végrehajtás
kindle-updater --dry-run  # csak a terv, semmit nem módosít
kindle-updater --yes      # megerősítés nélkül
kindle-updater --verbose  # minden érintett fájl listázva
kindle-updater --kindle /run/media/$USER/Kindle  # kézzel megadott Kindle
```

A program automatikusan megkeresi a csatlakoztatott Kindle-t a GVfs-en keresztül
(MTP vagy USB háttértár). Ha a Kindle csatlakoztatva van, de nincs felcsatolva,
megpróbálja maga felcsatolni.

## Hogyan dönt

- **Összehasonlítás:** név és méret alapján. MTP-n a Kindle nem őrzi meg a fájlok
  módosítási idejét, ezért az időbélyeg nem használható. A program az
  `~/.local/state/kindle-updater/state.json` fájlban megjegyzi, melyik változatot
  töltötte fel utoljára, így az azonos méretű, de megváltozott könyveket is frissíti.
- **Olvasási adatok (`.sdr`):** a Kindle a könyvek mellé `.sdr` mappát tesz
  (olvasási pozíció, kiemelések, jegyzetek). A megmaradó könyvekéhez a program nem
  nyúl, a törölt könyvekét törli.
- **Rejtett fájlok:** a ponttal kezdődő fájlokat és mappákat mindkét oldalon kihagyja.
- **Sorrend:** előbb töröl (helyet szabadít fel), aztán feltölt. Feltöltés előtt
  ellenőrzi, van-e elég szabad hely, feltöltés után pedig a fájl méretét.
- **Biztonsági fék:** ha egy forrásmappa üres, de a Kindle-ön vannak könyvek a
  párjában, a program leáll, ahelyett hogy mindent törölne.
- **Hibák:** egy fájl hibája nem állítja le a futást. A végén összesítve kiírja a
  hibákat, és nem nulla kilépési kóddal tér vissza. Újrafuttatáskor a hiányzó lépéseket
  pótolja.

## Telepítés

Szükséges: Rust, GLib/GIO és GVfs (MTP-hez a `gvfs-mtp` csomag).

```sh
cargo install --path .
```

# SyncTube na Linuxu

1. Instaliraj mpv (svira zvuk):
   - Ubuntu / Debian / Mint: `sudo apt install mpv`
   - Fedora: `sudo dnf install mpv` (iz RPM Fusion repozitorijuma)
   - Arch / Manjaro: `sudo pacman -S mpv`
2. Raspakuj i pokreni:
   ```
   tar -xzf synctube-linux-x86_64.tar.gz
   cd synctube
   ./synctube
   ```
3. Upiši ime sobe (isto kao ostali) i svoj nadimak, pa klikni **Poveži se**.

yt-dlp se sam preuzme pri prvom pokretanju u `~/.config/synctube/bin/`.
Podešavanja i log su u `~/.config/synctube/`.

## Build from source (optional)

```
sudo pacman -S --needed rust mpv
git clone https://github.com/hcr2accc-cpu/synctube
cd synctube
cargo build --release -p client --bin synctube
./target/release/synctube
```

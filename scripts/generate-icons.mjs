import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const run = (command, args) => execFileSync(command, args, { cwd: root, stdio: 'inherit' });
const temporary = mkdtempSync(join(tmpdir(), 'player-icons-'));
try {
  run('magick', ['-version']);
  // Call the CLI directly: the package's tauri wrapper also synchronizes versions.
  run('pnpm', ['exec', 'tauri', 'icon', 'public/app-icon.svg']);
  const master = resolve(root, 'src-tauri/icons/icon.png');
  const artwork = join(temporary, 'artwork.png');
  run('magick', [master, '-trim', '+repage', artwork]);
  const resources = resolve(root, 'src-tauri/gen/android/app/src/main/res');
  for (const [density, scale] of [['mdpi', 1], ['hdpi', 1.5], ['xhdpi', 2], ['xxhdpi', 3], ['xxxhdpi', 4]]) {
    const directory = join(resources, `mipmap-${density}`);
    // A 46dp square fits entirely inside Android's central 66dp safe circle
    // (46 * sqrt(2) < 66), including during adaptive-icon motion.
    const foreground = Math.floor(46 * scale);
    const canvas = Math.round(108 * scale);
    run('magick', [artwork, '-resize', `${foreground}x${foreground}`, '-gravity', 'center', '-background', 'none', '-extent', `${canvas}x${canvas}`, join(directory, 'ic_launcher_foreground.png')]);
    const legacy = Math.round(48 * scale);
    const size = Math.floor(legacy * 0.64);
    const launcher = join(directory, 'ic_launcher.png');
    run('magick', [artwork, '-resize', `${size}x${size}`, '-gravity', 'center', '-background', 'none', '-extent', `${legacy}x${legacy}`, launcher]);
    copyFileSync(launcher, join(directory, 'ic_launcher_round.png'));
  }
  writeFileSync(join(resources, 'values/ic_launcher_background.xml'), '<?xml version="1.0" encoding="utf-8"?>\n<resources>\n  <color name="ic_launcher_background">#00000000</color>\n</resources>\n');
  copyFileSync(join(resources, 'mipmap-anydpi-v26/ic_launcher.xml'), join(resources, 'mipmap-anydpi-v26/ic_launcher_round.xml'));
  const manifest = resolve(root, 'src-tauri/gen/android/app/src/main/AndroidManifest.xml');
  const xml = readFileSync(manifest, 'utf8');
  if (!xml.includes('android:roundIcon=')) {
    writeFileSync(manifest, xml.replace('android:icon="@mipmap/ic_launcher"', 'android:icon="@mipmap/ic_launcher"\n        android:roundIcon="@mipmap/ic_launcher_round"'));
  }
} finally {
  rmSync(temporary, { recursive: true, force: true });
}

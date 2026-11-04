"""Package a release build: the app (on macOS, ephys-rs.app), the readme and licence, build details,
dependency licence notices and checksums, zipped."""
import argparse
import hashlib
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import tomllib


def macos_app(exe, output):
    """A minimal app bundle around the executable, with the icon, signed ad hoc."""
    contents = output / "Contents"
    (contents / "MacOS").mkdir(parents=True)
    (contents / "Resources").mkdir()
    shutil.copy2(exe, contents / "MacOS" / "ephys-rs")
    with tempfile.TemporaryDirectory() as folder:
        iconset = Path(folder) / "icon.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
                pixels = str(size * scale)
                subprocess.run(["sips", "-z", pixels, pixels, "assets/icon-1024.png", "--out", str(iconset / name)],
                               check=True, capture_output=True)
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(contents / "Resources" / "icon.icns")],
                       check=True)
    version = tomllib.loads(Path("example/Cargo.toml").read_text())["package"]["version"]
    with (contents / "Info.plist").open("wb") as stream:
        plistlib.dump({
            "CFBundleExecutable": "ephys-rs",
            "CFBundleIdentifier": "org.chenneuroscience.ephys-rs",
            "CFBundleName": "ephys-rs",
            "CFBundleDisplayName": "ephys-rs",
            "CFBundleIconFile": "icon",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "NSHighResolutionCapable": True,
            "LSMinimumSystemVersion": "11.0",
        }, stream)
    subprocess.run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", str(output)], check=True)


def notices():
    """The licence files of every dependency, from the Cargo registry."""
    registry = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "registry" / "src"
    found = []
    for dependency in tomllib.loads(Path("Cargo.lock").read_text())["package"]:
        if not dependency.get("source", "").startswith("registry+"):
            continue
        name = f"{dependency['name']}-{dependency['version']}"
        for source in registry.glob(f"*/{name}"):
            for notice in sorted(source.iterdir()):
                if notice.is_file() and notice.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE")):
                    found.append(f"\n{'=' * 72}\n{name}/{notice.name}\n{'=' * 72}\n"
                                 + notice.read_text(encoding="utf-8", errors="replace"))
    if not found:
        raise RuntimeError("Dependency licence notices were not found in the Cargo registry")
    return "".join(found)


def package(exe, output, platform):
    output.mkdir(parents=True)
    if platform == "macos":
        macos_app(exe, output / "ephys-rs.app")
    else:
        shutil.copy2(exe, output / "ephys-rs.exe")
        # Its symbols, so crash reports' backtraces name functions and lines.
        # (Keeping its name: the executable says to look for that.)
        shutil.copy2(exe.with_name("ephys_rs.pdb"), output / "ephys_rs.pdb")
    for name in ("README.md", "LICENSE"):
        shutil.copy2(name, output / name)
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    compiler = subprocess.check_output(["rustc", "-vV"], text=True)
    def minor(manifest):
        return tomllib.loads(Path(manifest).read_text(encoding="utf-8"))["package"]["version"].split(".")[1]

    versions = f"Version: v0.{minor('example/Cargo.toml')}\nCore version: ephys-rs v0.{minor('Cargo.toml')}"
    (output / "BUILD.txt").write_text(f"{versions}\nSource commit: {commit}\n\n{compiler}", encoding="utf-8")
    (output / "THIRD_PARTY_NOTICES.txt").write_text(notices(), encoding="utf-8")
    checksums = [f"{hashlib.sha256(item.read_bytes()).hexdigest()}  {item.name}\n"
                 for item in sorted(output.iterdir()) if item.is_file()]
    (output / "SHA256SUMS.txt").write_text("".join(checksums), encoding="utf-8")
    archive = output.with_suffix(".zip")
    if platform == "macos":
        # ditto keeps the app bundle's permissions and signature intact.
        subprocess.run(["/usr/bin/ditto", "-c", "-k", "--keepParent", str(output), str(archive)], check=True)
    else:
        shutil.make_archive(str(output), "zip", output.parent, output.name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".zip.sha256").write_text(f"{digest}  {archive.name}\n")
    print(f"Packaged the {platform} build in {archive}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--platform", choices=("windows", "macos"), required=True)
    args = parser.parse_args()
    package(args.exe, args.output, args.platform)

"""Package a release build: the app (on macOS, ephys-rs.app), what the acquisition boards need, the readme and
licence, build details, dependency licence notices and checksums, zipped."""
import argparse
import hashlib
import io
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import tomllib
import urllib.request
import zipfile

# What the boards need, as the Open Ephys GUI has them: Opal Kelly's FrontPanel library, liboni's FT600 driver and
# the Open Ephys bitfiles, from the acquisition board plugin's release; and on Windows, the USB drivers and the C++
# runtime its installer sets up. Pinned by checksum.
PLUGIN = "https://openephys.jfrog.io/artifactory/AcquisitionBoard-plugin/{platform}/AcquisitionBoard-{platform}_2.0.0-API10.zip"
PLUGIN_SHA256 = {
    "mac": "02d285dad279b2c6beb647ceb5b4cea7330671f7570e807d09079dcbc881348c",
    "windows": "1efecb669e2f6c693842e70722d6e5648704e6d1c162669847a7331a6ef2b4af",
}
GUI_FILES = "https://raw.githubusercontent.com/open-ephys/plugin-GUI/560c995500f7f1438c6c3646311ff11776da49bb/Resources/DLLs/"
WINDOWS_DRIVERS = {
    "WU_FTD3XX_Driver/FTD3XXWU.Inf": "536270005c3a04d8ce26788bacd82e2c1c68e7acf4a18af59bb91479b7f408a3",
    "WU_FTD3XX_Driver/FTD3XXWU.cat": "09a32f93f8b3fdbaf68419a7ddab84454ce7dfe03dc382dc3257571e2160cef8",
    "FrontPanelUSB-DriverOnly-4.5.5.exe": "34e5f2b075dfd5fe3138747b26191444221ea83a3db38841668579b9de6bc03b",
}
INSTALL_DRIVERS = r"""@echo off
rem Installs the USB drivers the acquisition boards need, as the Open Ephys GUI's installer does: FTDI's D3XX driver,
rem for the generation 3 Open Ephys board, and Opal Kelly's FrontPanel driver, for the earlier boards and Intan's.
net session >nul 2>&1 || (powershell -NoProfile -Command "Start-Process -Verb RunAs '%~f0'" & exit /b)
pnputil /add-driver "%~dp0drivers\FTD3XXWU.Inf" /install
"%~dp0drivers\FrontPanelUSB-DriverOnly-4.5.5.exe"
pause
"""


def fetch(url, sha256):
    """The file at `url`, if its checksum is `sha256`."""
    data = urllib.request.urlopen(url, timeout=120).read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != sha256:
        raise RuntimeError(f"{url} has checksum {digest}, not {sha256}")
    return data


def board_files(platform, libraries, bitfiles):
    """The acquisition board plugin's shared files: libraries into `libraries`, bitfiles into `bitfiles`."""
    name = "mac" if platform == "macos" else platform
    archive = zipfile.ZipFile(io.BytesIO(fetch(PLUGIN.format(platform=name), PLUGIN_SHA256[name])))
    found = []
    for entry in archive.infolist():
        path = Path(entry.filename.replace("\\", "/"))
        if path.parent != Path("shared") or entry.is_dir():
            continue
        target = bitfiles if path.suffix == ".bit" else libraries
        target.mkdir(parents=True, exist_ok=True)
        (target / path.name).write_bytes(archive.read(entry))
        found.append(path.name)
    if not any(n.endswith(".bit") for n in found) or not any("FrontPanel" in n for n in found):
        raise RuntimeError(f"The acquisition board plugin's release has only {found}")


def windows_extras(output):
    """The USB drivers and a script to install them, and the C++ runtime the board libraries use."""
    drivers = output / "drivers"
    drivers.mkdir()
    for path, sha256 in WINDOWS_DRIVERS.items():
        (drivers / Path(path).name).write_bytes(fetch(GUI_FILES + path, sha256))
    (output / "Install USB drivers.cmd").write_bytes(INSTALL_DRIVERS.replace("\n", "\r\n").encode("ascii"))
    vswhere = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft Visual Studio/Installer/vswhere.exe"
    studio = subprocess.check_output([str(vswhere), "-latest", "-property", "installationPath"], text=True).strip()
    runtimes = sorted((Path(studio) / "VC/Redist/MSVC").glob("*/x64/Microsoft.VC14*.CRT"))
    if not runtimes:
        raise RuntimeError(f"The C++ runtime was not found in {studio}")
    for name in ("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
        shutil.copy2(runtimes[-1] / name, output / name)


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
    version = tomllib.loads(Path("examples/chen-neuro/Cargo.toml").read_text())["package"]["version"]
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
    board_files("macos", contents / "Frameworks", contents / "Resources")
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
    boards = (
        "What the acquisition boards need comes from the Open Ephys acquisition board plugin's release\n"
        "(github.com/open-ephys-plugins/acquisition-board, GPL-3.0) as the Open Ephys GUI has it: Opal Kelly's\n"
        "FrontPanel library, redistributed as Open Ephys does; liboni's FT600 driver, which includes FTDI's D3XX\n"
        "library; and the Open Ephys bitfiles, of Intan's Rhythm firmware. On Windows, FTDI's and Opal Kelly's USB\n"
        "drivers and Microsoft's Visual C++ runtime come too, as the Open Ephys GUI's installer has them.\n"
    )
    liboni = (f"\n{'=' * 72}\nliboni (github.com/open-ephys/liboni), ported in ephys-rs\n{'=' * 72}\n"
              + Path("src/standard_components/ephys/board/liboni.LICENSE").read_text())
    return boards + liboni + "".join(found)


def package(exe, output, platform):
    output.mkdir(parents=True)
    if platform == "macos":
        macos_app(exe, output / "ephys-rs.app")
    else:
        shutil.copy2(exe, output / "ephys-rs.exe")
        # Its symbols, so crash reports' backtraces name functions and lines.
        # (Keeping its name: the executable says to look for that.)
        shutil.copy2(exe.with_name("ephys_rs.pdb"), output / "ephys_rs.pdb")
        board_files("windows", output, output)
        windows_extras(output)
    for name in ("README.md", "LICENSE"):
        shutil.copy2(name, output / name)
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    compiler = subprocess.check_output(["rustc", "-vV"], text=True)
    def minor(manifest):
        return tomllib.loads(Path(manifest).read_text(encoding="utf-8"))["package"]["version"].split(".")[1]

    versions = f"Version: v0.{minor('examples/chen-neuro/Cargo.toml')}\nCore version: ephys-rs v0.{minor('Cargo.toml')}"
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

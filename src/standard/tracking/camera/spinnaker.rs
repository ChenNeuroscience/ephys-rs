//! Teledyne FLIR cameras (Blackfly, Chameleon, Grasshopper...) via the Spinnaker SDK, which is what Bonsai's
//! `SpinnakerCapture` uses. The SDK's C library is loaded at runtime, so building needs nothing from FLIR and
//! machines without the SDK just get an error when this camera is selected. Where it's looked for first is a global
//! setting (`SpinnakerSettings`).
//!
//! Function signatures follow the SDK's `SpinnakerC.h` (cross-checked against the generated bindings in
//! Spinnaker.jl). Exposure and gain are only changed when the tracker window asks; other settings (trigger
//! mode, region of interest...) are left as configured, e.g. in SpinView.

// Written before unsafe calls within `unsafe fn`s needed their own `unsafe` blocks (Rust 2024).
#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::BTreeSet;
use std::ffi::{CStr, CString, c_char, c_void};
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use eframe::egui;
use libloading::Library;
use serde::{Deserialize, Serialize};

use super::{Camera, CameraControls, FrameInfo, Range};
use super::{CameraSettings, Control};
use crate::global_settings::GlobalSettings;
use crate::search_folders::{self, SearchFolders};
use crate::theme::Colours;

type Handle = *mut c_void;
type SpinError = i32;
type Bool8 = u8;

const SUCCESS: SpinError = 0;
const ERR_TIMEOUT: SpinError = -1011;
/// How long `next_frame` waits, so the capture thread can notice when it should stop.
const GRAB_TIMEOUT_MS: u64 = 100;

#[cfg(windows)]
const LIBRARY_NAME: &str = "SpinnakerC_v140.dll";
#[cfg(target_os = "macos")]
const LIBRARY_NAME: &str = "libSpinnaker_C.dylib";
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_NAME: &str = "libSpinnaker_C.so";

/// The serial numbers of the cameras open, or still closing. Stopping lets go of a camera in the background, and playing
/// again doesn't wait, but the SDK gives everyone the same camera: closing it under a new run's feet would crash.
static IN_USE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Whether what's asked for is taken, by what's `in_use`: `serial`, or with none, any camera, as it can't be told which
/// would be found without asking the SDK, which shouldn't be touched while a camera's closing. (So having two cameras
/// takes their serial numbers.)
fn taken(in_use: &BTreeSet<String>, serial: Option<&str>) -> bool {
    serial.map_or(!in_use.is_empty(), |serial| in_use.contains(serial.trim()))
}

/// The SDK's system, and how many are using it. There's one between all the cameras, let go of when the last has
/// finished with it: letting go of it with a camera still open crashes the SDK.
struct System {
    handle: usize,
    users: usize,
}

static SYSTEM: Mutex<System> = Mutex::new(System { handle: 0, users: 0 });

impl System {
    fn get(api: &Api) -> io::Result<Handle> {
        let mut system = SYSTEM.lock().unwrap();
        if system.users == 0 {
            let mut handle: Handle = std::ptr::null_mut();
            unsafe { api.check((api.spinSystemGetInstance)(&mut handle), "getting the system")? };
            system.handle = handle as usize;
        }
        system.users += 1;
        Ok(system.handle as Handle)
    }

    fn let_go(api: &Api) {
        let mut system = SYSTEM.lock().unwrap();
        system.users -= 1;
        if system.users == 0 {
            step("Camera: letting go of the Spinnaker system");
            unsafe { (api.spinSystemReleaseInstance)(system.handle as Handle) };
        }
    }
}

/// Says what's about to be done to the camera, somewhere that survives the SDK crashing while it's done.
fn step(text: &str) {
    crate::crash::note(text);
    log::info!("{text}");
}

/// Where the Spinnaker SDK's library is looked for, for every Spinnaker camera: a tab in Settings….
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SpinnakerSettings {
    /// Looked in first.
    pub folders: SearchFolders,
}

impl SpinnakerSettings {
    /// Where the library's looked for: the folders chosen, the system's search path, then where the SDK's installers
    /// put it.
    fn candidates(&self) -> Vec<PathBuf> {
        let mut candidates = self.folders.candidates(&[LIBRARY_NAME], &[]);
        candidates.push(LIBRARY_NAME.into());
        candidates.extend(install_dirs().into_iter().map(|d| d.join(LIBRARY_NAME)));
        candidates
    }
}

impl GlobalSettings for SpinnakerSettings {
    const NAME: &'static str = "Spinnaker cameras";

    fn ui(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        search_folders::note(
            ui,
            colours,
            "Where to look first for Teledyne FLIR's Spinnaker SDK, for FLIR cameras: before where its installer puts \
             it. The SDK's installed from Teledyne FLIR.",
        );
        self.folders.ui(ui, colours);
        ui.add_space(8.0);
        let found = self.candidates().into_iter().find(|p| p.is_file());
        search_folders::found_ui(ui, colours, &[("Spinnaker library", found)]);
        ui.add_space(4.0);
        search_folders::note(ui, colours, search_folders::LOADED_ONCE);
        ui.add_space(8.0);
        search_folders::get_them_ui(
            ui,
            colours,
            &[("Teledyne FLIR's Spinnaker SDK", "https://www.teledynevisionsolutions.com/products/spinnaker-sdk/")],
        );
    }
}

/// Where the SDK installers put the library, tried after the system search path.
fn install_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        let program_files = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
        for vendor in ["Teledyne", "FLIR Systems", "Point Grey Research"] {
            dirs.push([&program_files, vendor, "Spinnaker", "bin64", "vs2015"].iter().collect());
        }
    } else if cfg!(target_os = "macos") {
        dirs.push("/usr/local/lib".into());
        dirs.push("/Applications/Spinnaker/lib".into());
    } else {
        dirs.push("/opt/spinnaker/lib".into());
        dirs.push("/usr/lib".into());
    }
    dirs
}

macro_rules! spinnaker_api {
    ($($name:ident($($arg:ty),*);)*) => {
        #[allow(non_snake_case)]
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) -> SpinError,)*
            _library: Library,
        }

        impl Api {
            unsafe fn from_library(library: Library) -> Result<Api, String> {
                Ok(Api {
                    $($name: *library
                        .get::<unsafe extern "C" fn($($arg),*) -> SpinError>(stringify!($name))
                        .map_err(|e| format!("Spinnaker library is missing {}: {e}", stringify!($name)))?,)*
                    _library: library,
                })
            }
        }
    };
}

spinnaker_api! {
    spinErrorGetLastMessage(*mut c_char, *mut usize);
    spinSystemGetInstance(*mut Handle);
    spinSystemReleaseInstance(Handle);
    spinSystemGetCameras(Handle, Handle);
    spinCameraListCreateEmpty(*mut Handle);
    spinCameraListGetSize(Handle, *mut usize);
    spinCameraListGet(Handle, usize, *mut Handle);
    spinCameraListClear(Handle);
    spinCameraListDestroy(Handle);
    spinCameraInit(Handle);
    spinCameraDeInit(Handle);
    spinCameraRelease(Handle);
    spinCameraGetNodeMap(Handle, *mut Handle);
    spinCameraGetTLDeviceNodeMap(Handle, *mut Handle);
    spinCameraBeginAcquisition(Handle);
    spinCameraEndAcquisition(Handle);
    spinCameraGetNextImageEx(Handle, u64, *mut Handle);
    spinImageIsIncomplete(Handle, *mut Bool8);
    spinImageGetWidth(Handle, *mut usize);
    spinImageGetHeight(Handle, *mut usize);
    spinImageGetStride(Handle, *mut usize);
    spinImageGetBitsPerPixel(Handle, *mut usize);
    spinImageGetData(Handle, *mut *mut c_void);
    spinImageGetTimeStamp(Handle, *mut u64);
    spinImageGetFrameID(Handle, *mut u64);
    spinImageChunkDataGetIntValue(Handle, *const c_char, *mut i64);
    spinImageRelease(Handle);
    spinNodeMapGetNode(Handle, *const c_char, *mut Handle);
    spinNodeIsAvailable(Handle, *mut Bool8);
    spinNodeIsReadable(Handle, *mut Bool8);
    spinNodeIsWritable(Handle, *mut Bool8);
    spinEnumerationGetEntryByName(Handle, *const c_char, *mut Handle);
    spinEnumerationEntryGetIntValue(Handle, *mut i64);
    spinEnumerationSetIntValue(Handle, i64);
    spinBooleanSetValue(Handle, Bool8);
    spinFloatSetValue(Handle, f64);
    spinFloatGetValue(Handle, *mut f64);
    spinFloatGetMin(Handle, *mut f64);
    spinFloatGetMax(Handle, *mut f64);
    spinStringGetValue(Handle, *mut c_char, *mut usize);
    spinNodeToString(Handle, *mut c_char, *mut usize);
}

impl Api {
    /// Loads the library once it's found (it's never unloaded); until then, tries again each time.
    fn get(settings: &SpinnakerSettings) -> io::Result<&'static Api> {
        static API: OnceLock<Api> = OnceLock::new();
        if let Some(api) = API.get() {
            return Ok(api);
        }
        let api = Api::load(&settings.candidates()).map_err(|e| io::Error::new(io::ErrorKind::NotFound, e))?;
        Ok(API.get_or_init(|| api))
    }

    fn load(candidates: &[PathBuf]) -> Result<Api, String> {
        let mut errors = Vec::new();
        for candidate in candidates {
            match unsafe { open_library(candidate) } {
                Ok(library) => {
                    let api = unsafe { Api::from_library(library) }?;
                    log::info!("Camera: Spinnaker SDK loaded from {}", candidate.display());
                    crate::crash::catch_native_again();
                    return Ok(api);
                }
                Err(e) => errors.push(format!("{}: {e}", candidate.display())),
            }
        }
        Err(format!("Spinnaker SDK not found (install it from Teledyne FLIR). Tried {}", errors.join("; ")))
    }

    fn check(&self, err: SpinError, what: &str) -> io::Result<()> {
        if err == SUCCESS {
            return Ok(());
        }
        let mut buf = [0 as c_char; 1024];
        let mut len = buf.len();
        let message = unsafe {
            if (self.spinErrorGetLastMessage)(buf.as_mut_ptr(), &mut len) == SUCCESS {
                CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
            } else {
                String::new()
            }
        };
        Err(io::Error::other(format!("Spinnaker {what} failed (error {err}) {message}")))
    }

    fn flag(&self, f: unsafe extern "C" fn(Handle, *mut Bool8) -> SpinError, node: Handle) -> bool {
        let mut v: Bool8 = 0;
        unsafe { f(node, &mut v) == SUCCESS && v != 0 }
    }

    /// The named node, if the camera has it and it's available.
    fn node(&self, map: Handle, name: &str) -> Option<Handle> {
        let name = CString::new(name).ok()?;
        let mut node: Handle = std::ptr::null_mut();
        let ok = unsafe { (self.spinNodeMapGetNode)(map, name.as_ptr(), &mut node) } == SUCCESS;
        (ok && !node.is_null() && self.flag(self.spinNodeIsAvailable, node)).then_some(node)
    }

    fn writable(&self, map: Handle, name: &str) -> io::Result<Handle> {
        self.node(map, name)
            .filter(|&n| self.flag(self.spinNodeIsWritable, n))
            .ok_or_else(|| io::Error::other(format!("camera setting {name} isn't available or writable")))
    }

    fn set_enum(&self, map: Handle, name: &str, entry: &str) -> io::Result<()> {
        let node = self.writable(map, name)?;
        let entry_name = CString::new(entry).unwrap();
        let mut entry_node: Handle = std::ptr::null_mut();
        let mut value = 0i64;
        unsafe {
            self.check((self.spinEnumerationGetEntryByName)(node, entry_name.as_ptr(), &mut entry_node), name)?;
            if entry_node.is_null() || !self.flag(self.spinNodeIsReadable, entry_node) {
                return Err(io::Error::other(format!("camera doesn't support {name} = {entry}")));
            }
            self.check((self.spinEnumerationEntryGetIntValue)(entry_node, &mut value), name)?;
            self.check((self.spinEnumerationSetIntValue)(node, value), name)
        }
    }

    fn set_bool(&self, map: Handle, name: &str, value: bool) -> io::Result<()> {
        let node = self.writable(map, name)?;
        unsafe { self.check((self.spinBooleanSetValue)(node, value as Bool8), name) }
    }

    fn set_float(&self, map: Handle, name: &str, value: f64) -> io::Result<()> {
        let node = self.writable(map, name)?;
        unsafe { self.check((self.spinFloatSetValue)(node, value), name) }
    }

    fn get_float(&self, map: Handle, name: &str) -> Option<Range> {
        let node = self.node(map, name).filter(|&n| self.flag(self.spinNodeIsReadable, n))?;
        let (mut min, mut max, mut current) = (0.0, 0.0, 0.0);
        unsafe {
            ((self.spinFloatGetMin)(node, &mut min) == SUCCESS
                && (self.spinFloatGetMax)(node, &mut max) == SUCCESS
                && (self.spinFloatGetValue)(node, &mut current) == SUCCESS)
                .then_some(Range { min, max, current })
        }
    }

    /// Sets e.g. ExposureAuto/ExposureTime from a [`Control`].
    fn set_control(&self, map: Handle, auto_name: &str, value_name: &str, control: Control) -> io::Result<()> {
        match control {
            Control::Unchanged => Ok(()),
            Control::Auto => self.set_enum(map, auto_name, "Continuous"),
            Control::Manual(v) => {
                // Not every camera has the auto node; that's fine if the value itself is writable.
                let _ = self.set_enum(map, auto_name, "Off");
                let range = self.get_float(map, value_name);
                self.set_float(map, value_name, range.map_or(v, |r| v.clamp(r.min, r.max)))
            }
        }
    }

    /// The named setting's value, of any type, as text.
    fn as_text(&self, map: Handle, name: &str) -> Option<String> {
        let node = self.node(map, name).filter(|&n| self.flag(self.spinNodeIsReadable, n))?;
        let mut buf = [0 as c_char; 256];
        let mut len = buf.len();
        unsafe {
            ((self.spinNodeToString)(node, buf.as_mut_ptr(), &mut len) == SUCCESS)
                .then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        }
    }

    fn get_string(&self, map: Handle, name: &str) -> Option<String> {
        let node = self.node(map, name).filter(|&n| self.flag(self.spinNodeIsReadable, n))?;
        let mut buf = [0 as c_char; 256];
        let mut len = buf.len();
        unsafe {
            ((self.spinStringGetValue)(node, buf.as_mut_ptr(), &mut len) == SUCCESS)
                .then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        }
    }
}

/// What's worth knowing about how a camera's set up, when working out why it isn't working.
const REPORTED: [&str; 15] = [
    "WidthMax",
    "HeightMax",
    "Width",
    "Height",
    "OffsetX",
    "OffsetY",
    "PixelFormat",
    "AcquisitionMode",
    "TriggerMode",
    "TriggerSource",
    "AcquisitionFrameRate",
    "ExposureAuto",
    "ExposureTime",
    "GainAuto",
    "Gain",
];

unsafe fn open_library(path: &std::path::Path) -> Result<Library, libloading::Error> {
    #[cfg(windows)]
    {
        // With a full path, also look for Spinnaker's own DLLs in that directory.
        use libloading::os::windows;
        windows::Library::load_with_flags(path, windows::LOAD_WITH_ALTERED_SEARCH_PATH).map(Into::into)
    }
    #[cfg(not(windows))]
    {
        Library::new(path)
    }
}

pub struct SpinnakerCamera {
    api: &'static Api,
    system: Handle,
    camera: Handle,
    /// The camera's settings (GenICam node map); valid once initialised.
    map: Handle,
    /// Camera clock timestamps from chunk data (as Bonsai used), rather than the transport layer's.
    chunk_timestamps: bool,
    /// The frame rate last asked for, and whether the camera accepted it.
    frame_rate: f64,
    frame_rate_set: bool,
    description: String,
    /// Its serial number, in `IN_USE`, once it's found.
    serial: Option<String>,
    /// How many frames have come incomplete.
    incomplete: u64,
    /// Things that didn't go to plan when configuring the camera, for the window to show.
    warnings: Vec<String>,
    /// The frame fetched, until it's copied out: the SDK's image, where its pixels are, and their width, height
    /// and stride.
    fetched: Option<(Image<'static>, *const u8, usize, usize, usize)>,
}

// The handles are only used from the thread that owns the camera.
unsafe impl Send for SpinnakerCamera {}

impl SpinnakerCamera {
    /// Opens the camera with the given serial number, or the first one found. `fps` sets the camera's frame
    /// rate where it allows that; `settings` say where the SDK's looked for. `note` is told how it's getting on.
    pub fn open(
        serial: Option<&str>,
        fps: f64,
        settings: &SpinnakerSettings,
        note: &mut dyn FnMut(String),
    ) -> io::Result<Self> {
        if taken(&IN_USE.lock().unwrap(), serial) {
            return Err(io::Error::new(io::ErrorKind::ResourceBusy, "it's in use, or still closing"));
        }
        let api = Api::get(settings)?;
        let system = System::get(api)?;
        // From here on, Drop cleans up whatever has been set up.
        let mut cam = SpinnakerCamera {
            api,
            system,
            camera: std::ptr::null_mut(),
            map: std::ptr::null_mut(),
            chunk_timestamps: false,
            frame_rate: fps,
            frame_rate_set: false,
            description: String::new(),
            serial: None,
            incomplete: 0,
            warnings: Vec::new(),
            fetched: None,
        };
        cam.camera = cam.find(serial, note)?;
        note(format!("Camera: opening {}", cam.description));
        cam.configure(fps)?;
        let reported = REPORTED.iter().filter_map(|name| Some(format!("{name} {}", api.as_text(cam.map, name)?)));
        note(format!("Camera: configured: {}", reported.collect::<Vec<_>>().join(", ")));
        note("Camera: starting acquisition".into());
        unsafe { api.check((api.spinCameraBeginAcquisition)(cam.camera), "starting acquisition")? };
        Ok(cam)
    }

    fn find(&mut self, serial: Option<&str>, note: &mut dyn FnMut(String)) -> io::Result<Handle> {
        let api = self.api;
        let mut list: Handle = std::ptr::null_mut();
        let mut found: Handle = std::ptr::null_mut();
        let mut seen = Vec::new();
        unsafe {
            api.check((api.spinCameraListCreateEmpty)(&mut list), "creating the camera list")?;
            let result = (|| -> io::Result<()> {
                api.check((api.spinSystemGetCameras)(self.system, list), "listing cameras")?;
                let mut count = 0usize;
                api.check((api.spinCameraListGetSize)(list, &mut count), "listing cameras")?;
                for i in 0..count {
                    let mut camera: Handle = std::ptr::null_mut();
                    api.check((api.spinCameraListGet)(list, i, &mut camera), "getting a camera")?;
                    let mut tl_map: Handle = std::ptr::null_mut();
                    let (this_serial, model) =
                        if (api.spinCameraGetTLDeviceNodeMap)(camera, &mut tl_map) == SUCCESS && !tl_map.is_null() {
                            let get = |name| api.get_string(tl_map, name).unwrap_or_default();
                            (get("DeviceSerialNumber"), get("DeviceModelName"))
                        } else {
                            Default::default()
                        };
                    let wanted = serial.is_none_or(|s| s.trim() == this_serial);
                    if found.is_null() && wanted && IN_USE.lock().unwrap().insert(this_serial.clone()) {
                        found = camera;
                        self.description = format!("{model} (serial {this_serial})");
                        self.serial = Some(this_serial.clone());
                    } else {
                        (api.spinCameraRelease)(camera);
                    }
                    seen.push(format!("{model} (serial {this_serial})"));
                }
                Ok(())
            })();
            (api.spinCameraListClear)(list);
            (api.spinCameraListDestroy)(list);
            result?;
        }
        note(format!("Camera: Spinnaker sees {} camera(s): {}", seen.len(), seen.join(", ")));
        if found.is_null() {
            let in_use = IN_USE.lock().unwrap();
            let closing = !in_use.is_empty() && serial.is_none_or(|s| in_use.contains(s.trim()));
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                match serial {
                    _ if closing => format!("it's in use, or still closing (found: {})", seen.join(", ")),
                    Some(s) => format!("no Spinnaker camera with serial {s} (found: {})", seen.join(", ")),
                    None => "no Spinnaker cameras found (is it plugged in, and powered?)".into(),
                },
            ));
        }
        Ok(found)
    }

    fn configure(&mut self, fps: f64) -> io::Result<()> {
        let api = self.api;
        let mut map: Handle = std::ptr::null_mut();
        unsafe {
            api.check((api.spinCameraInit)(self.camera), "initialising the camera (is it open in another program?)")?;
            api.check((api.spinCameraGetNodeMap)(self.camera, &mut map), "getting camera settings")?;
        }
        self.map = map;
        api.set_enum(map, "PixelFormat", "Mono8")
            .map_err(|e| io::Error::other(format!("camera must support 8-bit mono output: {e}")))?;
        let _ = api.set_enum(map, "AcquisitionMode", "Continuous");

        let mut notes = Vec::new();
        self.chunk_timestamps = api.set_bool(map, "ChunkModeActive", true).is_ok()
            && api.set_enum(map, "ChunkSelector", "Timestamp").is_ok()
            && api.set_bool(map, "ChunkEnable", true).is_ok();
        if !self.chunk_timestamps {
            notes.push("Camera has no chunk timestamps; using transport timestamps.".to_string());
        }
        // Older models (Chameleon3, Flea3, Grasshopper3) only allow setting the rate with its auto mode off,
        // and call the switch ...Enabled; Blackfly S calls it ...Enable.
        let _ = api.set_enum(map, "AcquisitionFrameRateAuto", "Off");
        let _ = api
            .set_bool(map, "AcquisitionFrameRateEnable", true)
            .or_else(|_| api.set_bool(map, "AcquisitionFrameRateEnabled", true));
        self.frame_rate_set = match api.set_float(map, "AcquisitionFrameRate", fps) {
            Ok(()) => true,
            Err(e) => {
                notes.push(format!("Couldn't set the frame rate ({e}); it's whatever the camera is configured for."));
                false
            }
        };
        self.warnings = notes;
        Ok(())
    }
}

/// Releases an image back to the SDK when dropped.
struct Image<'a>(&'a Api, Handle);

impl Drop for Image<'_> {
    fn drop(&mut self) {
        unsafe { (self.0.spinImageRelease)(self.1) };
    }
}

impl Camera for SpinnakerCamera {
    fn fetch(&mut self) -> io::Result<Option<FrameInfo>> {
        self.fetched = None;
        let api = self.api;
        let mut handle: Handle = std::ptr::null_mut();
        let err = unsafe { (api.spinCameraGetNextImageEx)(self.camera, GRAB_TIMEOUT_MS, &mut handle) };
        if err == ERR_TIMEOUT {
            return Ok(None);
        }
        api.check(err, "grabbing a frame")?;
        let image = Image(api, handle);

        if api.flag(api.spinImageIsIncomplete, image.1) {
            // A dropped frame; the gap in frame ids is counted by the caller.
            self.incomplete += 1;
            if self.incomplete.is_power_of_two() {
                let count = self.incomplete;
                log::warn!(
                    "Camera: {count} incomplete frame(s) so far: is it getting enough USB or network bandwidth?"
                );
            }
            return Ok(None);
        }
        let (mut width, mut height, mut stride, mut bits) = (0usize, 0usize, 0usize, 0usize);
        let mut data: *mut c_void = std::ptr::null_mut();
        let (mut frame_id, mut timestamp_ns) = (0u64, 0u64);
        unsafe {
            api.check((api.spinImageGetWidth)(image.1, &mut width), "reading the frame")?;
            api.check((api.spinImageGetHeight)(image.1, &mut height), "reading the frame")?;
            api.check((api.spinImageGetStride)(image.1, &mut stride), "reading the frame")?;
            api.check((api.spinImageGetBitsPerPixel)(image.1, &mut bits), "reading the frame")?;
            api.check((api.spinImageGetData)(image.1, &mut data), "reading the frame")?;
            api.check((api.spinImageGetFrameID)(image.1, &mut frame_id), "reading the frame")?;
            let mut chunk_ts = 0i64;
            let name = c"ChunkTimestamp";
            if !(self.chunk_timestamps
                && (api.spinImageChunkDataGetIntValue)(image.1, name.as_ptr(), &mut chunk_ts) == SUCCESS)
            {
                api.check((api.spinImageGetTimeStamp)(image.1, &mut timestamp_ns), "reading the frame")?;
            } else {
                timestamp_ns = chunk_ts as u64;
            }
        }
        if bits != 8 || data.is_null() || stride < width {
            return Err(io::Error::other(format!("unexpected frame: {bits} bits/pixel, stride {stride}")));
        }

        let info = FrameInfo {
            id: frame_id,
            timestamp: timestamp_ns as f64 * 1e-9,
            width: width as u32,
            height: height as u32,
        };
        self.fetched = Some((image, data as *const u8, width, height, stride));
        Ok(Some(info))
    }

    fn copy_to(&mut self, into: Option<&mut [u8]>) {
        let Some((image, data, width, height, stride)) = self.fetched.take() else { return };
        if let Some(into) = into {
            // Row by row, since rows may be padded. Safety: the SDK's image is there until it's dropped, below.
            let src = unsafe { std::slice::from_raw_parts(data, stride * height) };
            for (row, out) in src.chunks_exact(stride).zip(into.chunks_exact_mut(width)) {
                out.copy_from_slice(&row[..width]);
            }
        }
        drop(image);
    }

    fn description(&self) -> String {
        format!("Spinnaker: {}", self.description)
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    fn apply(&mut self, settings: &CameraSettings) -> io::Result<()> {
        let api = self.api;
        // Not an error if it won't: what it's doing about it is reported as it opens, and in `controls`.
        if settings.frame_rate != self.frame_rate {
            self.frame_rate = settings.frame_rate;
            self.frame_rate_set = api.set_float(self.map, "AcquisitionFrameRate", self.frame_rate).is_ok();
        }
        let exposure = api.set_control(self.map, "ExposureAuto", "ExposureTime", settings.exposure);
        let gain = api.set_control(self.map, "GainAuto", "Gain", settings.gain);
        exposure.and(gain)
    }

    fn controls(&self) -> CameraControls {
        let api = self.api;
        CameraControls {
            frame_rate_set: self.frame_rate_set,
            frame_rate: api.get_float(self.map, "AcquisitionFrameRate"),
            exposure: api.get_float(self.map, "ExposureTime"),
            gain: api.get_float(self.map, "Gain"),
        }
    }
}

impl Drop for SpinnakerCamera {
    fn drop(&mut self) {
        self.fetched = None; // the image, back to the SDK first
        let api = self.api;
        unsafe {
            if !self.camera.is_null() {
                step("Camera: stopping acquisition");
                (api.spinCameraEndAcquisition)(self.camera);
                step("Camera: closing");
                (api.spinCameraDeInit)(self.camera);
                (api.spinCameraRelease)(self.camera);
            }
        }
        System::let_go(api);
        if let Some(serial) = &self.serial {
            IN_USE.lock().unwrap().remove(serial);
            step("Camera: closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_in_use_is_left_alone() {
        let in_use = BTreeSet::from(["123".to_string()]);
        assert!(taken(&in_use, Some("123")) && taken(&in_use, Some(" 123 ")));
        assert!(!taken(&in_use, Some("456")), "another camera");
        assert!(taken(&in_use, None), "the first found might be it");
        assert!(!taken(&BTreeSet::new(), None));
    }

    #[test]
    fn missing_sdk_is_a_clear_error() {
        let err = Api::load(&["/nonexistent/libSpinnaker_C".into()]).err().unwrap();
        assert!(err.contains("Spinnaker SDK not found"), "{err}");
    }

    #[test]
    fn folders_chosen_are_looked_in_first_then_the_system_then_the_installers() {
        let settings = SpinnakerSettings { folders: SearchFolders(vec!["/chosen".into()]) };
        let candidates = settings.candidates();
        assert_eq!(candidates[0], PathBuf::from("/chosen").join(LIBRARY_NAME));
        assert_eq!(candidates[1], PathBuf::from(LIBRARY_NAME));
        assert_eq!(candidates[2..], install_dirs().into_iter().map(|d| d.join(LIBRARY_NAME)).collect::<Vec<_>>()[..]);
    }
}

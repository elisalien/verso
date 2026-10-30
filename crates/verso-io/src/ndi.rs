//! NDI® sender, and a minimal finder/receiver used to check it.
//!
//! The NDI runtime library is loaded dynamically: from `NDI_RUNTIME_DIR_V6`
//! (or V5), else from the library search path. Declarations follow the
//! public NDI C API.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use libloading::Library;

use crate::{Error, Result};

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
}

const FOURCC_RGBA: u32 = fourcc(b'R', b'G', b'B', b'A');
const FRAME_FORMAT_PROGRESSIVE: i32 = 1;
/// Lets the SDK make a timecode.
const TIMECODE_SYNTHESIZE: i64 = i64::MAX;
const RECV_COLOR_RGBX_RGBA: i32 = 2;
const RECV_BANDWIDTH_HIGHEST: i32 = 100;
const FRAME_TYPE_VIDEO: i32 = 1;

#[repr(C)]
struct SendCreate {
    p_ndi_name: *const c_char,
    p_groups: *const c_char,
    clock_video: bool,
    clock_audio: bool,
}

#[repr(C)]
struct VideoFrameV2 {
    xres: i32,
    yres: i32,
    four_cc: u32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    picture_aspect_ratio: f32,
    frame_format_type: i32,
    timecode: i64,
    p_data: *mut u8,
    line_stride_in_bytes: i32,
    p_metadata: *const c_char,
    timestamp: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Source {
    p_ndi_name: *const c_char,
    p_url_address: *const c_char,
}

#[repr(C)]
struct FindCreate {
    show_local_sources: bool,
    p_groups: *const c_char,
    p_extra_ips: *const c_char,
}

#[repr(C)]
struct RecvCreateV3 {
    source_to_connect_to: Source,
    color_format: i32,
    bandwidth: i32,
    allow_video_fields: bool,
    p_ndi_recv_name: *const c_char,
}

type Instance = *mut c_void;

/// Entry points of the runtime.
struct Api {
    _library: Library,
    send_create: unsafe extern "C" fn(*const SendCreate) -> Instance,
    send_send_video_v2: unsafe extern "C" fn(Instance, *const VideoFrameV2),
    send_get_no_connections: unsafe extern "C" fn(Instance, u32) -> i32,
    send_destroy: unsafe extern "C" fn(Instance),
    find_create_v2: unsafe extern "C" fn(*const FindCreate) -> Instance,
    find_wait_for_sources: unsafe extern "C" fn(Instance, u32) -> bool,
    find_get_current_sources: unsafe extern "C" fn(Instance, *mut u32) -> *const Source,
    find_destroy: unsafe extern "C" fn(Instance),
    recv_create_v3: unsafe extern "C" fn(*const RecvCreateV3) -> Instance,
    recv_capture_v2:
        unsafe extern "C" fn(Instance, *mut VideoFrameV2, *mut c_void, *mut c_void, u32) -> i32,
    recv_free_video_v2: unsafe extern "C" fn(Instance, *const VideoFrameV2),
    recv_destroy: unsafe extern "C" fn(Instance),
}

fn library_paths() -> Vec<std::path::PathBuf> {
    let file = if cfg!(windows) {
        "Processing.NDI.Lib.x64.dll"
    } else if cfg!(target_os = "macos") {
        "libndi.dylib"
    } else {
        "libndi.so.6"
    };
    let mut paths: Vec<_> = ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| std::path::PathBuf::from(dir).join(file))
        .collect();
    if cfg!(target_os = "macos") {
        paths.push("/Library/NDI SDK for Apple/lib/macOS/libndi.dylib".into());
    }
    paths.push(file.into());
    paths
}

fn load() -> std::result::Result<Api, String> {
    let mut last_error =
        String::from("NDI runtime not found; install NDI Tools or the NDI runtime");
    for path in library_paths() {
        // SAFETY: loading the vendor runtime; its initializers have no
        // preconditions.
        let library = match unsafe { Library::new(&path) } {
            Ok(library) => library,
            Err(e) => {
                last_error = format!("{}: {e}", path.display());
                continue;
            }
        };
        // SAFETY: the declared signatures match the NDI C API.
        unsafe {
            macro_rules! sym {
                ($name:literal) => {
                    *library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|e| format!("{}: {e}", $name))?
                };
            }
            let initialize: unsafe extern "C" fn() -> bool = sym!("NDIlib_initialize");
            if !initialize() {
                return Err("this CPU is not supported by NDI".into());
            }
            return Ok(Api {
                send_create: sym!("NDIlib_send_create"),
                send_send_video_v2: sym!("NDIlib_send_send_video_v2"),
                send_get_no_connections: sym!("NDIlib_send_get_no_connections"),
                send_destroy: sym!("NDIlib_send_destroy"),
                find_create_v2: sym!("NDIlib_find_create_v2"),
                find_wait_for_sources: sym!("NDIlib_find_wait_for_sources"),
                find_get_current_sources: sym!("NDIlib_find_get_current_sources"),
                find_destroy: sym!("NDIlib_find_destroy"),
                recv_create_v3: sym!("NDIlib_recv_create_v3"),
                recv_capture_v2: sym!("NDIlib_recv_capture_v2"),
                recv_free_video_v2: sym!("NDIlib_recv_free_video_v2"),
                recv_destroy: sym!("NDIlib_recv_destroy"),
                _library: library,
            });
        }
    }
    Err(last_error)
}

/// The runtime, loaded once per process.
fn api() -> Result<Arc<Api>> {
    static API: OnceLock<std::result::Result<Arc<Api>, String>> = OnceLock::new();
    API.get_or_init(|| load().map(Arc::new))
        .clone()
        .map_err(Error::Ndi)
}

/// Whether the NDI runtime can be loaded.
pub fn available() -> bool {
    api().is_ok()
}

/// Publishes frames as an NDI source named after `name` on this machine.
pub struct NdiSender {
    api: Arc<Api>,
    instance: Instance,
    name: String,
}

// SAFETY: NDI send instances may be used from any thread, one at a time.
unsafe impl Send for NdiSender {}

impl NdiSender {
    pub fn new(name: &str) -> Result<Self> {
        let api = api()?;
        let c_name = CString::new(name).map_err(|e| Error::Ndi(e.to_string()))?;
        let create = SendCreate {
            p_ndi_name: c_name.as_ptr(),
            p_groups: std::ptr::null(),
            // Verso paces frames itself.
            clock_video: false,
            clock_audio: false,
        };
        // SAFETY: `create` points to valid strings for the call.
        let instance = unsafe { (api.send_create)(&create) };
        if instance.is_null() {
            return Err(Error::Ndi(format!("could not create sender {name}")));
        }
        Ok(Self {
            api,
            instance,
            name: name.to_owned(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sends one RGBA8 frame, `width * height * 4` bytes, rows top to bottom.
    /// The call returns once NDI has copied the data.
    pub fn send(&mut self, width: u32, height: u32, rgba: &[u8], fps: u32) {
        assert_eq!(rgba.len(), (width * height * 4) as usize, "frame size");
        let frame = VideoFrameV2 {
            xres: width as i32,
            yres: height as i32,
            four_cc: FOURCC_RGBA,
            frame_rate_n: fps.max(1) as i32,
            frame_rate_d: 1,
            picture_aspect_ratio: 0.0,
            frame_format_type: FRAME_FORMAT_PROGRESSIVE,
            timecode: TIMECODE_SYNTHESIZE,
            p_data: rgba.as_ptr().cast_mut(),
            line_stride_in_bytes: (width * 4) as i32,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY: the synchronous send reads `rgba` before returning.
        unsafe { (self.api.send_send_video_v2)(self.instance, &frame) };
    }

    /// Receivers connected now.
    pub fn connections(&self) -> usize {
        // SAFETY: valid instance; 0 means do not wait.
        unsafe { (self.api.send_get_no_connections)(self.instance, 0).max(0) as usize }
    }
}

impl Drop for NdiSender {
    fn drop(&mut self) {
        // SAFETY: created by `send_create`, destroyed once.
        unsafe { (self.api.send_destroy)(self.instance) };
    }
}

/// Names of the NDI sources seen within `wait`.
pub fn find_sources(wait: Duration) -> Result<Vec<String>> {
    let api = api()?;
    let create = FindCreate {
        show_local_sources: true,
        p_groups: std::ptr::null(),
        p_extra_ips: std::ptr::null(),
    };
    // SAFETY: valid create struct; the finder is destroyed below.
    unsafe {
        let finder = (api.find_create_v2)(&create);
        if finder.is_null() {
            return Err(Error::Ndi("could not create finder".into()));
        }
        (api.find_wait_for_sources)(finder, wait.as_millis() as u32);
        let mut count = 0;
        let sources = (api.find_get_current_sources)(finder, &mut count);
        let names = (0..count as usize)
            .map(|i| {
                CStr::from_ptr((*sources.add(i)).p_ndi_name)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        (api.find_destroy)(finder);
        Ok(names)
    }
}

/// Receives one RGBA frame from the source named `source`, waiting up to
/// `wait`. Used to check senders.
pub fn receive_frame(source: &str, wait: Duration) -> Result<(u32, u32, Vec<u8>)> {
    let api = api()?;
    let c_source = CString::new(source).map_err(|e| Error::Ndi(e.to_string()))?;
    let create = RecvCreateV3 {
        source_to_connect_to: Source {
            p_ndi_name: c_source.as_ptr(),
            p_url_address: std::ptr::null(),
        },
        color_format: RECV_COLOR_RGBX_RGBA,
        bandwidth: RECV_BANDWIDTH_HIGHEST,
        allow_video_fields: false,
        p_ndi_recv_name: std::ptr::null(),
    };
    let deadline = std::time::Instant::now() + wait;
    // SAFETY: valid structs; the receiver and frames are released below.
    unsafe {
        let receiver = (api.recv_create_v3)(&create);
        if receiver.is_null() {
            return Err(Error::Ndi("could not create receiver".into()));
        }
        let result = loop {
            let mut frame: VideoFrameV2 = std::mem::zeroed();
            let kind = (api.recv_capture_v2)(
                receiver,
                &mut frame,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                200,
            );
            if kind == FRAME_TYPE_VIDEO {
                let (w, h) = (frame.xres as u32, frame.yres as u32);
                let stride = frame.line_stride_in_bytes as usize;
                let mut data = Vec::with_capacity((w * h * 4) as usize);
                for row in 0..h as usize {
                    let line =
                        std::slice::from_raw_parts(frame.p_data.add(row * stride), w as usize * 4);
                    data.extend_from_slice(line);
                }
                (api.recv_free_video_v2)(receiver, &frame);
                break Ok((w, h, data));
            }
            if std::time::Instant::now() > deadline {
                break Err(Error::Ndi(format!("no frame from {source}")));
            }
        };
        (api.recv_destroy)(receiver);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_layouts_match_the_c_api() {
        // Sizes of the 64-bit C structs.
        assert_eq!(size_of::<SendCreate>(), 24);
        assert_eq!(size_of::<VideoFrameV2>(), 72);
        assert_eq!(size_of::<Source>(), 16);
        assert_eq!(size_of::<FindCreate>(), 24);
        assert_eq!(size_of::<RecvCreateV3>(), 40);
    }

    #[test]
    fn sender_round_trip() {
        if !available() {
            eprintln!("NDI runtime not installed, skipping");
            return;
        }
        let name = format!("Verso Test {}", std::process::id());
        let mut sender = NdiSender::new(&name).unwrap();
        let sources = find_sources(Duration::from_secs(5)).unwrap();
        let full_name = sources
            .iter()
            .find(|s| s.ends_with(&format!("({name})")))
            .unwrap_or_else(|| panic!("{name} not in {sources:?}"))
            .clone();

        // Keep sending while a receiver connects and captures.
        let (w, h) = (64, 32);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        for pixel in frame.chunks_mut(4) {
            pixel.copy_from_slice(&[255, 64, 0, 255]);
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sending = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    sender.send(w, h, &frame, 30);
                    std::thread::sleep(Duration::from_millis(30));
                }
            })
        };
        let received = receive_frame(&full_name, Duration::from_secs(10));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        sending.join().unwrap();

        let (rw, rh, data) = received.unwrap();
        assert_eq!((rw, rh), (w, h));
        let center = ((h / 2 * w + w / 2) * 4) as usize;
        let pixel = &data[center..center + 4];
        // NDI transports in YUV: allow a small color error.
        for (got, want) in pixel.iter().zip([255u8, 64, 0, 255]) {
            assert!(got.abs_diff(want) <= 12, "{pixel:?}");
        }
    }
}

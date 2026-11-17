//! Spout sender (Windows): shares the output as a DirectX 11 texture that
//! Spout receivers (Resolume, TouchDesigner, OBS with the Spout plugin, …)
//! read without copying through the CPU on their side.
//!
//! Implements the Spout 2 sharing protocol directly, so no Spout DLL is
//! needed:
//!
//! - a D3D11 texture created with `D3D11_RESOURCE_MISC_SHARED`, BGRA8;
//! - shared memory named after the sender holding its `SharedTextureInfo`
//!   (share handle, size, format);
//! - the name added to the `SpoutSenderNames` list (256-byte entries,
//!   guarded by the `SpoutSenderNames_mutex` mutex), and set as the active
//!   sender when there is none;
//! - `<name>_SpoutAccessMutex` held while the texture is written.
//!
//! Frames come from the renderer as RGBA8; they are uploaded with
//! `UpdateSubresource`, so this path costs one CPU copy per frame.
//!
//! [`SpoutReceiver`] reads the other way: it looks the sender up by name,
//! opens its shared texture on a device of its own, copies it to a staging
//! texture and reads it back as RGBA8 on a background thread, so received
//! frames take the same CPU path into the renderer as NDI® frames.

use std::ffi::c_void;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED,
    WAIT_OBJECT_0,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_B8G8R8X8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R16G16B16A16_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, IDXGIAdapter, IDXGIAdapter1,
    IDXGIFactory6, IDXGIResource,
};
use windows::Win32::System::Memory::{
    CreateFileMappingA, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_BASIC_INFORMATION,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingA, PAGE_READWRITE, UnmapViewOfFile,
    VirtualQuery,
};
use windows::Win32::System::Threading::{CreateMutexA, ReleaseMutex, WaitForSingleObject};
use windows::core::{Interface, PCSTR};

use crate::receiver::{FrameReceiver, Shared};
use crate::{Error, Result};

const NAME_LEN: usize = 256;
/// Default size of the sender list (Spout's "MaxSenders").
const MAX_SENDERS: usize = 64;

/// Per-sender record read by receivers.
#[repr(C)]
struct SharedTextureInfo {
    share_handle: u32,
    width: u32,
    height: u32,
    format: u32,
    usage: u32,
    description: [u16; 128],
    partner_id: u32,
}

fn error(context: &str, e: impl std::fmt::Display) -> Error {
    Error::Spout(format!("{context}: {e}"))
}

fn cstr(text: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = text.bytes().filter(|&b| b != 0).collect();
    bytes.push(0);
    bytes
}

/// A named file mapping with its view, closed on drop.
struct SharedMemory {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    size: usize,
}

impl SharedMemory {
    fn open(name: &str, size: usize) -> Result<Self> {
        let name = cstr(name);
        // SAFETY: valid NUL-terminated name; the handle and view are
        // released in Drop.
        unsafe {
            let handle = CreateFileMappingA(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                size as u32,
                PCSTR(name.as_ptr()),
            )
            .map_err(|e| error("shared memory", e))?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return Err(Error::Spout("could not map shared memory".into()));
            }
            Ok(Self { handle, view, size })
        }
    }

    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the view maps `size` bytes for the life of `self`.
        unsafe { std::slice::from_raw_parts_mut(self.view.Value as *mut u8, self.size) }
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        // SAFETY: created in `open`, released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

/// A named mutex, closed on drop.
struct Mutex(HANDLE);

impl Mutex {
    fn open(name: &str) -> Result<Self> {
        let name = cstr(name);
        // SAFETY: valid name; closed in Drop.
        unsafe {
            CreateMutexA(None, false, PCSTR(name.as_ptr()))
                .map(Mutex)
                .map_err(|e| error("mutex", e))
        }
    }

    /// Opens the mutex `name`, and tells whether another handle to it was
    /// already open somewhere. A named mutex goes away with the last process
    /// holding it, crash included, so this tells whether its owner is alive.
    fn open_was_held(name: &str) -> Result<(Self, bool)> {
        let name = cstr(name);
        // SAFETY: valid name; closed in Drop. The last error is read right
        // after the call that sets it.
        unsafe {
            let mutex = CreateMutexA(None, false, PCSTR(name.as_ptr()))
                .map(Mutex)
                .map_err(|e| error("mutex", e))?;
            Ok((mutex, GetLastError() == ERROR_ALREADY_EXISTS))
        }
    }

    /// Runs `f` while holding the mutex (or without it after 50 ms, as
    /// Spout does, rather than stall the frame).
    fn with<T>(&self, f: impl FnOnce() -> T) -> T {
        // SAFETY: valid handle.
        // Abandoned (its last owner crashed) is still acquired: it must be
        // released, or every other Spout program waits on Verso.
        let wait = unsafe { WaitForSingleObject(self.0, 50) };
        let locked = wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED;
        let result = f();
        if locked {
            // SAFETY: locked above by this thread.
            unsafe {
                let _ = ReleaseMutex(self.0);
            }
        }
        result
    }
}

impl Drop for Mutex {
    fn drop(&mut self) {
        // SAFETY: created in `open`, closed once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Reads the names in a sender list buffer.
fn read_names(buffer: &[u8]) -> Vec<String> {
    buffer
        .chunks(NAME_LEN)
        .map(|entry| {
            let end = entry.iter().position(|&b| b == 0).unwrap_or(entry.len());
            String::from_utf8_lossy(&entry[..end]).into_owned()
        })
        .take_while(|name| !name.is_empty())
        .collect()
}

/// Writes `names` into a sender list buffer, clearing the rest.
fn write_names(buffer: &mut [u8], names: &[String]) {
    buffer.fill(0);
    for (entry, name) in buffer.chunks_mut(NAME_LEN).zip(names) {
        let bytes = name.as_bytes();
        let len = bytes.len().min(NAME_LEN - 1);
        entry[..len].copy_from_slice(&bytes[..len]);
    }
}

/// What Verso writes in the description of its sender records: the path of
/// the executable, as Spout's own senders do.
fn own_description() -> [u16; 128] {
    let mut description = [0u16; 128];
    if let Ok(exe) = std::env::current_exe() {
        for (slot, unit) in description
            .iter_mut()
            .take(127)
            .zip(exe.to_string_lossy().encode_utf16())
        {
            *slot = unit;
        }
    }
    description
}

/// The high-performance GPU, as Windows ranks them.
///
/// On laptops with two GPUs the default adapter is often the integrated one,
/// while Verso's renderer and receivers such as Resolume run on the discrete
/// one. A texture shared from one GPU cannot be opened on the other, so the
/// receiver gets nothing. Using the high-performance GPU puts the shared
/// texture where the renderer and receivers are.
fn preferred_adapter() -> Option<IDXGIAdapter> {
    // SAFETY: plain factory creation and enumeration.
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory1().ok()?;
        let adapter: IDXGIAdapter1 = factory
            .EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
            .ok()?;
        adapter.cast().ok()
    }
}

/// A hardware Direct3D 11 device on the high-performance GPU (the default
/// adapter when it cannot be found), and its immediate context.
fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    if let Some(adapter) = preferred_adapter()
        && let Ok(created) = create_device_on(Some(&adapter))
    {
        return Ok(created);
    }
    create_device_on(None)
}

fn create_device_on(adapter: Option<&IDXGIAdapter>) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // An explicit adapter needs the "unknown" driver type.
    let driver = if adapter.is_some() {
        D3D_DRIVER_TYPE_UNKNOWN
    } else {
        D3D_DRIVER_TYPE_HARDWARE
    };
    // SAFETY: out pointers are valid for the call.
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| error("Direct3D 11 device", e))?;
    }
    let device = device.ok_or_else(|| Error::Spout("no Direct3D 11 device".into()))?;
    let context = context.ok_or_else(|| Error::Spout("no Direct3D 11 context".into()))?;
    Ok((device, context))
}

/// A Spout sender. Registered while it exists.
pub struct SpoutSender {
    name: String,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    texture: Option<(ID3D11Texture2D, u32, u32)>,
    info: SharedMemory,
    names: SharedMemory,
    names_mutex: Mutex,
    access: Mutex,
    /// Exists while this sender does, see [`Mutex::open_was_held`].
    _alive: Mutex,
    bgra: Vec<u8>,
}

// SAFETY: D3D11 devices are free-threaded; the context is used only by the
// thread that owns the sender.
unsafe impl Send for SpoutSender {}

impl SpoutSender {
    pub fn new(name: &str) -> Result<Self> {
        if name.is_empty() || name.len() >= NAME_LEN {
            return Err(Error::Spout("sender name must be 1–255 characters".into()));
        }
        let (device, context) = create_device()?;

        let names_mutex = Mutex::open("SpoutSenderNames_mutex")?;
        let mut names = SharedMemory::open("SpoutSenderNames", MAX_SENDERS * NAME_LEN)?;
        let (alive, was_alive) = Mutex::open_was_held(&format!("Verso_SpoutSender_{name}"))?;
        let registered = names_mutex.with(|| {
            let mut list = read_names(names.bytes());
            if list.iter().any(|n| n == name) {
                // A Verso that crashed leaves its name in the list, and a
                // receiver still open keeps its record alive. The name is
                // taken back when it was this program's and no running
                // Verso holds it; any other sender keeps it.
                let ours = sender_info(name).is_none_or(|i| i.description == own_description());
                if ours && !was_alive {
                    return Ok(());
                }
                return Err(Error::Spout(format!(
                    "a sender named {name} already exists"
                )));
            }
            if list.len() >= MAX_SENDERS {
                return Err(Error::Spout("too many Spout senders".into()));
            }
            list.push(name.to_owned());
            write_names(names.bytes(), &list);
            Ok(())
        });
        registered?;
        let info = SharedMemory::open(name, size_of::<SharedTextureInfo>())?;
        let access = Mutex::open(&format!("{name}_SpoutAccessMutex"))?;
        // Become the active sender when no other is.
        if let Ok(mut active) = SharedMemory::open("ActiveSenderName", NAME_LEN)
            && read_names(active.bytes()).is_empty()
        {
            write_names(active.bytes(), &[name.to_owned()]);
            // The mapping must stay open for receivers to see it: leak it
            // for the life of the process, as Spout's own senders do.
            std::mem::forget(active);
        }
        Ok(Self {
            name: name.to_owned(),
            device,
            context,
            texture: None,
            info,
            names,
            names_mutex,
            access,
            _alive: alive,
            bgra: Vec::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// (Re)creates the shared texture and publishes its handle and size.
    fn texture(&mut self, width: u32, height: u32) -> Result<ID3D11Texture2D> {
        if let Some((texture, w, h)) = &self.texture
            && (*w, *h) == (width, height)
        {
            return Ok(texture.clone());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
        };
        let mut texture = None;
        // SAFETY: valid description and out pointer.
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|e| error("shared texture", e))?;
        }
        let texture = texture.ok_or_else(|| Error::Spout("no shared texture".into()))?;
        let resource: IDXGIResource = texture.cast().map_err(|e| error("DXGI resource", e))?;
        // SAFETY: valid shared resource.
        let handle = unsafe { resource.GetSharedHandle() }.map_err(|e| error("share handle", e))?;
        let description = own_description();
        let record = SharedTextureInfo {
            // Shared handles fit in 32 bits, which is what the protocol stores.
            share_handle: handle.0 as usize as u32,
            width,
            height,
            format: DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
            usage: 0,
            description,
            partner_id: 0,
        };
        let bytes = self.info.bytes();
        // SAFETY: the mapping is exactly one `SharedTextureInfo` long.
        unsafe { std::ptr::write_unaligned(bytes.as_mut_ptr() as *mut SharedTextureInfo, record) };
        self.texture = Some((texture.clone(), width, height));
        Ok(texture)
    }

    /// Publishes one RGBA8 frame, `width * height * 4` bytes, top row first.
    pub fn send(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
        if rgba.len() != width as usize * height as usize * 4 {
            return Err(Error::Spout("frame size does not match".into()));
        }
        let texture = self.texture(width, height)?;
        self.bgra.clear();
        self.bgra.extend(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], p[3]]),
        );
        let (context, bgra) = (&self.context, &self.bgra);
        self.access.with(|| {
            // SAFETY: the texture matches the frame size; the data outlives
            // the call, which copies it.
            unsafe {
                context.UpdateSubresource(
                    &texture,
                    0,
                    None,
                    bgra.as_ptr() as *const c_void,
                    width * 4,
                    0,
                );
                // Make the update visible to other devices now.
                context.Flush();
            }
        });
        Ok(())
    }
}

impl Drop for SpoutSender {
    fn drop(&mut self) {
        let (name, names) = (&self.name, &mut self.names);
        self.names_mutex.with(|| {
            let mut list = read_names(names.bytes());
            list.retain(|n| n != name);
            write_names(names.bytes(), &list);
        });
    }
}

// ---------------------------------------------------------------------------
// Receiving

/// Largest received frame side; bigger textures are refused.
const MAX_RECEIVED: u32 = 16384;

/// Read-only view of a mapping some other sender created, closed on drop.
struct SharedView {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    size: usize,
}

impl SharedView {
    /// Opens the mapping named `name`; `None` when it does not exist.
    fn open(name: &str) -> Option<Self> {
        let name = cstr(name);
        // SAFETY: valid NUL-terminated name; the handle and view are
        // released in Drop.
        unsafe {
            let handle = OpenFileMappingA(FILE_MAP_READ.0, false, PCSTR(name.as_ptr())).ok()?;
            let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return None;
            }
            let mut info = MEMORY_BASIC_INFORMATION::default();
            let queried = VirtualQuery(
                Some(view.Value),
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            let size = if queried == 0 { 0 } else { info.RegionSize };
            Some(Self { handle, view, size })
        }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the view maps at least `size` bytes (whole pages) for the
        // life of `self`.
        unsafe { std::slice::from_raw_parts(self.view.Value as *const u8, self.size) }
    }
}

impl Drop for SharedView {
    fn drop(&mut self) {
        // SAFETY: opened in `open`, released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

/// The texture record a sender publishes, if it is running.
fn sender_info(name: &str) -> Option<SharedTextureInfo> {
    let view = SharedView::open(name)?;
    let bytes = view.bytes();
    if bytes.len() < size_of::<SharedTextureInfo>() {
        return None;
    }
    // SAFETY: at least one record long; any bit pattern is a valid record.
    Some(unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const SharedTextureInfo) })
}

/// Names of the Spout senders running on this computer, in the order they
/// started. Senders that left their name behind without their texture
/// record (crashed) are skipped.
pub fn find_senders() -> Vec<String> {
    let Some(list) = SharedView::open("SpoutSenderNames") else {
        return Vec::new();
    };
    let names = match Mutex::open("SpoutSenderNames_mutex") {
        Ok(mutex) => mutex.with(|| read_names(list.bytes())),
        Err(_) => read_names(list.bytes()),
    };
    names
        .into_iter()
        .filter(|name| sender_info(name).is_some())
        .collect()
}

/// One received RGBA8 frame, rows top to bottom, straight alpha.
pub type SpoutFrame = crate::receiver::LiveFrame;

/// Receives the frames of one Spout sender on a background thread, keeping
/// only the newest. Waits for the sender when it is not running, and follows
/// it when it restarts or changes size. Only distinct pictures count as
/// frames. Stops when dropped.
pub type SpoutReceiver = FrameReceiver;

/// Starts receiving from the sender named `name` (as [`find_senders`]
/// lists it).
pub fn receiver(name: &str) -> Result<SpoutReceiver> {
    if name.is_empty() || name.len() >= NAME_LEN {
        return Err(Error::Spout("sender name must be 1–255 characters".into()));
    }
    let sender = name.to_owned();
    FrameReceiver::spawn("verso-spout-recv", move |shared| {
        receive_loop(&sender, shared)
    })
    .map_err(|e| error("thread", e))
}

/// The sender's texture opened on the receiving device, with a staging copy
/// the CPU reads.
struct Opened {
    share_handle: u32,
    texture: ID3D11Texture2D,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

/// Whether frames of `format` can be converted to RGBA8.
fn supported(format: DXGI_FORMAT) -> bool {
    [
        DXGI_FORMAT_B8G8R8A8_UNORM,
        DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        DXGI_FORMAT_B8G8R8X8_UNORM,
        DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        DXGI_FORMAT_R16G16B16A16_UNORM,
        DXGI_FORMAT_R16G16B16A16_FLOAT,
    ]
    .contains(&format)
}

fn open_texture(device: &ID3D11Device, info: &SharedTextureInfo) -> Result<Opened> {
    // Spout stores the 32-bit handle value; widen it as `LongToHandle` does.
    let handle = HANDLE(info.share_handle as i32 as isize as *mut c_void);
    let mut texture: Option<ID3D11Texture2D> = None;
    // SAFETY: the out pointer is valid; a stale handle makes the call fail.
    unsafe { device.OpenSharedResource(handle, &mut texture) }
        .map_err(|e| error("open shared texture", e))?;
    let texture = texture.ok_or_else(|| Error::Spout("no shared texture".into()))?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: valid out pointer.
    unsafe { texture.GetDesc(&mut desc) };
    if !supported(desc.Format) {
        return Err(Error::Spout(format!(
            "texture format {} is not supported",
            desc.Format.0
        )));
    }
    if desc.Width == 0
        || desc.Height == 0
        || desc.Width > MAX_RECEIVED
        || desc.Height > MAX_RECEIVED
    {
        return Err(Error::Spout(format!(
            "unusable texture size {}×{}",
            desc.Width, desc.Height
        )));
    }
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: desc.Width,
        Height: desc.Height,
        MipLevels: 1,
        ArraySize: 1,
        Format: desc.Format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging = None;
    // SAFETY: valid description and out pointer.
    unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }
        .map_err(|e| error("staging texture", e))?;
    let staging = staging.ok_or_else(|| Error::Spout("no staging texture".into()))?;
    Ok(Opened {
        share_handle: info.share_handle,
        texture,
        staging,
        width: desc.Width,
        height: desc.Height,
        format: desc.Format,
    })
}

/// IEEE half float to f32.
fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((h >> 10) & 0x1f);
    let mantissa = f32::from(h & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Converts one row of `format` pixels to RGBA8, appending to `out`.
fn convert_row(format: DXGI_FORMAT, row: &[u8], out: &mut Vec<u8>) {
    match format {
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => {
            out.extend(
                row.as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[2], p[1], p[0], p[3]]),
            );
        }
        DXGI_FORMAT_B8G8R8X8_UNORM => {
            out.extend(
                row.as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[2], p[1], p[0], 255]),
            );
        }
        DXGI_FORMAT_R16G16B16A16_UNORM => {
            // The high byte of each little-endian 16-bit channel.
            out.extend(row.as_chunks::<2>().0.iter().map(|c| c[1]));
        }
        DXGI_FORMAT_R16G16B16A16_FLOAT => {
            out.extend(row.as_chunks::<2>().0.iter().map(|c| {
                let v = half_to_f32(u16::from_le_bytes([c[0], c[1]]));
                (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            }));
        }
        _ => out.extend_from_slice(row),
    }
}

/// Bytes per pixel of the supported formats.
fn pixel_size(format: DXGI_FORMAT) -> usize {
    match format {
        DXGI_FORMAT_R16G16B16A16_UNORM | DXGI_FORMAT_R16G16B16A16_FLOAT => 8,
        _ => 4,
    }
}

/// Copies the sender's texture (holding its access mutex) and reads it back
/// as RGBA8.
fn read_frame(context: &ID3D11DeviceContext, access: &Mutex, opened: &Opened) -> Result<Vec<u8>> {
    access.with(|| {
        // SAFETY: both textures have the same size and format.
        unsafe { context.CopyResource(&opened.staging, &opened.texture) }
    });
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: staging texture with CPU read access; unmapped below.
    unsafe {
        context
            .Map(&opened.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| error("read texture", e))?;
    }
    let (w, h) = (opened.width as usize, opened.height as usize);
    let row_bytes = w * pixel_size(opened.format);
    let pitch = mapped.RowPitch as usize;
    let mut data = Vec::with_capacity(w * h * 4);
    if !mapped.pData.is_null() && pitch >= row_bytes {
        for y in 0..h {
            // SAFETY: a mapped texture holds `height` rows of `RowPitch` bytes.
            let row = unsafe {
                std::slice::from_raw_parts((mapped.pData as *const u8).add(y * pitch), row_bytes)
            };
            convert_row(opened.format, row, &mut data);
        }
    }
    // SAFETY: mapped above.
    unsafe { context.Unmap(&opened.staging, 0) };
    if data.len() != w * h * 4 {
        return Err(Error::Spout("could not read the texture".into()));
    }
    Ok(data)
}

fn receive_loop(name: &str, shared: &Shared) {
    use std::time::Duration;

    let (device, context) = match create_device() {
        Ok(d) => d,
        Err(e) => return shared.set_error(Some(e.to_string())),
    };
    let access = match Mutex::open(&format!("{name}_SpoutAccessMutex")) {
        Ok(m) => m,
        Err(e) => return shared.set_error(Some(e.to_string())),
    };
    let mut opened: Option<Opened> = None;
    let mut last: Vec<u8> = Vec::new();
    while !shared.stopped() {
        let Some(info) = sender_info(name) else {
            opened = None;
            shared.set_error(Some(format!("Waiting for the Spout sender {name}")));
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };
        if opened.as_ref().is_none_or(|o| {
            o.share_handle != info.share_handle || (o.width, o.height) != (info.width, info.height)
        }) {
            match open_texture(&device, &info) {
                Ok(o) => opened = Some(o),
                Err(e) => {
                    opened = None;
                    shared.set_error(Some(e.to_string()));
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
        }
        let Some(o) = &opened else { continue };
        match read_frame(&context, &access, o) {
            Ok(data) => {
                shared.set_error(None);
                // Only new pictures count as frames: Verso's sender, like
                // many others, does not signal frames.
                if data != last {
                    last.clone_from(&data);
                    shared.publish(SpoutFrame {
                        width: o.width,
                        height: o.height,
                        data,
                    });
                }
            }
            Err(e) => {
                opened = None;
                shared.set_error(Some(e.to_string()));
            }
        }
        // About 120 polls a second, enough for 60 fps senders.
        std::thread::sleep(Duration::from_millis(8));
    }
}

/// Receives one RGBA frame from the sender named `name`, waiting up to
/// `wait`. Used to check senders.
pub fn receive_frame(name: &str, wait: std::time::Duration) -> Result<SpoutFrame> {
    let mut receiver = receiver(name)?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        if let Some(frame) = receiver.take() {
            return Ok(frame);
        }
        if std::time::Instant::now() >= deadline {
            let message = receiver
                .error()
                .unwrap_or_else(|| format!("no frame from {name}"));
            return Err(Error::Spout(message));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_spout() {
        assert_eq!(size_of::<SharedTextureInfo>(), 280);
    }

    #[test]
    fn name_list_round_trip() {
        let mut buffer = vec![0u8; 4 * NAME_LEN];
        write_names(&mut buffer, &["Verso".into(), "Other".into()]);
        assert_eq!(read_names(&buffer), ["Verso", "Other"]);
    }

    #[test]
    fn half_floats() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0), 0.0);
    }

    #[test]
    fn rows_convert_to_rgba() {
        let mut out = Vec::new();
        convert_row(DXGI_FORMAT_B8G8R8A8_UNORM, &[1, 2, 3, 4], &mut out);
        convert_row(DXGI_FORMAT_B8G8R8X8_UNORM, &[1, 2, 3, 0], &mut out);
        convert_row(DXGI_FORMAT_R8G8B8A8_UNORM, &[1, 2, 3, 4], &mut out);
        assert_eq!(out, [3, 2, 1, 4, 3, 2, 1, 255, 1, 2, 3, 4]);
    }

    /// Verso's own sender, received by name. Needs a Direct3D 11 GPU, so it
    /// does not run by default: `cargo test -p verso-io -- --ignored spout`.
    #[test]
    #[ignore = "needs a Direct3D 11 GPU"]
    fn sender_round_trip() {
        let name = format!("Verso Spout Test {}", std::process::id());
        let mut sender = SpoutSender::new(&name).unwrap();
        let (w, h) = (64, 32);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        for (i, pixel) in frame.chunks_mut(4).enumerate() {
            pixel.copy_from_slice(&[255, 64, (i % 256) as u8, 255]);
        }
        sender.send(w, h, &frame).unwrap();
        let senders = find_senders();
        assert!(senders.contains(&name), "{name} not in {senders:?}");
        let received = receive_frame(&name, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!((received.width, received.height), (w, h));
        assert!(received.data == frame, "pixels differ");

        // A new picture arrives as a new frame.
        let mut receiver = receiver(&name).unwrap();
        assert!(receive_until(&mut receiver).data == frame);
        frame.fill(7);
        sender.send(w, h, &frame).unwrap();
        assert!(receive_until(&mut receiver).data == frame);
        assert_eq!(receiver.received(), 2);

        drop(sender);
        assert!(!find_senders().contains(&name));
    }

    /// A name left in the list by a Verso that crashed is taken back; a name
    /// a running sender holds is not.
    #[test]
    #[ignore = "needs a Direct3D 11 GPU"]
    fn stale_name_is_taken_back() {
        let name = format!("Verso Spout Stale {}", std::process::id());
        {
            let mutex = Mutex::open("SpoutSenderNames_mutex").unwrap();
            let mut names = SharedMemory::open("SpoutSenderNames", MAX_SENDERS * NAME_LEN).unwrap();
            mutex.with(|| {
                let mut list = read_names(names.bytes());
                list.push(name.clone());
                write_names(names.bytes(), &list);
            });
        }
        let sender = SpoutSender::new(&name).unwrap();
        assert!(
            SpoutSender::new(&name).is_err(),
            "took a live sender's name"
        );
        drop(sender);
        assert!(!find_senders().contains(&name));
    }

    fn receive_until(receiver: &mut SpoutReceiver) -> SpoutFrame {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(frame) = receiver.take() {
                return frame;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no frame: {:?}", receiver.error());
    }
}

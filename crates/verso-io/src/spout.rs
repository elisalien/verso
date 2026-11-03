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

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIResource;
use windows::Win32::System::Memory::{
    CreateFileMappingA, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::Threading::{CreateMutexA, ReleaseMutex, WaitForSingleObject};
use windows::core::{Interface, PCSTR};

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

    /// Runs `f` while holding the mutex (or without it after 50 ms, as
    /// Spout does, rather than stall the frame).
    fn with<T>(&self, f: impl FnOnce() -> T) -> T {
        // SAFETY: valid handle.
        let locked = unsafe { WaitForSingleObject(self.0, 50) } == WAIT_OBJECT_0;
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
        let mut device = None;
        let mut context = None;
        // SAFETY: out pointers are valid; the device lives in `self`.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
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

        let names_mutex = Mutex::open("SpoutSenderNames_mutex")?;
        let mut names = SharedMemory::open("SpoutSenderNames", MAX_SENDERS * NAME_LEN)?;
        let registered = names_mutex.with(|| {
            let mut list = read_names(names.bytes());
            if list.iter().any(|n| n == name) {
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
}

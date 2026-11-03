//! Syphon server (macOS): shares the output as a Metal texture with Syphon
//! clients (Resolume, MadMapper, VDMX, OBS with the Syphon plugin, …).
//!
//! `Syphon.framework` is loaded at run time, like the NDI runtime: from
//! `VERSO_SYPHON` (path to the framework binary), next to the executable
//! (`../Frameworks` in an app bundle) or `/Library/Frameworks`. Nothing is
//! linked at build time.
//!
//! Frames come from the renderer as RGBA8 and are uploaded into a Metal
//! texture, then published with `SyphonMetalServer`.

use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2::msg_send;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, ProtocolObject};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLOrigin,
    MTLPixelFormat, MTLRegion, MTLSize, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use crate::{Error, Result};

fn framework_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::env::var_os("VERSO_SYPHON")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.to_owned()))
    {
        paths.push(dir.join("../Frameworks/Syphon.framework/Syphon"));
        paths.push(dir.join("Syphon.framework/Syphon"));
    }
    paths.push("/Library/Frameworks/Syphon.framework/Syphon".into());
    paths
}

/// Loads the framework once; its classes then exist in the runtime.
fn load() -> std::result::Result<(), String> {
    static LOADED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            for path in framework_paths() {
                // SAFETY: loading the Syphon framework; its initialisers
                // only register Objective-C classes.
                if let Ok(library) = unsafe { libloading::Library::new(&path) } {
                    // Classes must stay registered: never unload.
                    std::mem::forget(library);
                    return Ok(());
                }
            }
            Err("Syphon.framework not found: install it in /Library/Frameworks".into())
        })
        .clone()
}

/// Whether Syphon can be used.
pub fn available() -> bool {
    load().is_ok() && AnyClass::get(c"SyphonMetalServer").is_some()
}

/// A Syphon server. Visible to clients while it exists.
pub struct SyphonServer {
    name: String,
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    server: Retained<AnyObject>,
    texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    bgra: Vec<u8>,
}

// SAFETY: Metal devices, queues and Syphon servers may be used from any
// thread; this one is only used by the thread that owns it.
unsafe impl Send for SyphonServer {}

impl SyphonServer {
    pub fn new(name: &str) -> Result<Self> {
        load().map_err(Error::Syphon)?;
        let class = AnyClass::get(c"SyphonMetalServer")
            .ok_or_else(|| Error::Syphon("this Syphon.framework has no Metal server".into()))?;
        let device = MTLCreateSystemDefaultDevice()
            .ok_or_else(|| Error::Syphon("no Metal device".into()))?;
        let queue = device
            .newCommandQueue()
            .ok_or_else(|| Error::Syphon("no Metal command queue".into()))?;
        let ns_name = NSString::from_str(name);
        // SAFETY: -[SyphonMetalServer initWithName:device:options:] takes an
        // NSString, an MTLDevice and an optional NSDictionary.
        let server: Option<Retained<AnyObject>> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![
                allocated,
                initWithName: &*ns_name,
                device: &*device,
                options: std::ptr::null::<AnyObject>()
            ]
        };
        let server = server.ok_or_else(|| Error::Syphon("could not create the server".into()))?;
        Ok(Self {
            name: name.to_owned(),
            device,
            queue,
            server,
            texture: None,
            bgra: Vec::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn texture(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Retained<ProtocolObject<dyn MTLTexture>>> {
        if let Some(texture) = &self.texture
            && (texture.width(), texture.height()) == (width as usize, height as usize)
        {
            return Ok(texture.clone());
        }
        // SAFETY: plain 2D texture description.
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::BGRA8Unorm,
                width as usize,
                height as usize,
                false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::ShaderRead);
        let texture = self
            .device
            .newTextureWithDescriptor(&descriptor)
            .ok_or_else(|| Error::Syphon("could not create the texture".into()))?;
        self.texture = Some(texture.clone());
        Ok(texture)
    }

    /// Publishes one RGBA8 frame, `width * height * 4` bytes, top row first.
    pub fn send(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
        if rgba.len() != width as usize * height as usize * 4 {
            return Err(Error::Syphon("frame size does not match".into()));
        }
        let texture = self.texture(width, height)?;
        self.bgra.clear();
        self.bgra
            .extend(rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]));
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: width as usize,
                height: height as usize,
                depth: 1,
            },
        };
        let bytes = NonNull::new(self.bgra.as_mut_ptr().cast())
            .ok_or_else(|| Error::Syphon("empty frame".into()))?;
        // SAFETY: the region matches the texture and `bytes` holds it.
        unsafe {
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region,
                0,
                bytes,
                width as usize * 4,
            );
        }
        let commands = self
            .queue
            .commandBuffer()
            .ok_or_else(|| Error::Syphon("no command buffer".into()))?;
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width as f64, height as f64),
        );
        // SAFETY: -[SyphonMetalServer publishFrameTexture:onCommandBuffer:
        // imageRegion:flipped:] with a texture of this device.
        unsafe {
            let _: () = msg_send![
                &*self.server,
                publishFrameTexture: &*texture,
                onCommandBuffer: &*commands,
                imageRegion: rect,
                flipped: Bool::NO
            ];
        }
        commands.commit();
        Ok(())
    }
}

impl Drop for SyphonServer {
    fn drop(&mut self) {
        // SAFETY: -[SyphonServerBase stop] takes no arguments.
        unsafe {
            let _: () = msg_send![&*self.server, stop];
        }
    }
}

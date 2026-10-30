//! GPU textures holding decoded images and video frames.

/// A decoded frame ready to be drawn, as passed to [`crate::Renderer::render`].
#[derive(Clone, Copy)]
pub struct MediaView<'a> {
    pub view: &'a wgpu::TextureView,
    pub size: (u32, u32),
}

/// An sRGB RGBA8 texture that frames are uploaded into. It is recreated when
/// the frame size changes.
pub struct MediaTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl MediaTexture {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let size = (size.0.max(1), size.1.max(1));
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("media"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            texture,
            view,
            size,
        }
    }

    /// Uploads RGBA8 `data` of `size`. Returns `true` when the texture had to be
    /// recreated, so views taken earlier are stale.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        data: &[u8],
    ) -> bool {
        let recreated = size != self.size;
        if recreated {
            *self = Self::new(device, size);
        }
        queue.write_texture(
            self.texture.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.0 * 4),
                rows_per_image: Some(size.1),
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        recreated
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn media_view(&self) -> MediaView<'_> {
        MediaView {
            view: &self.view,
            size: self.size,
        }
    }
}

//! GPU textures holding decoded images and video frames.
//!
//! Frames arrive either as RGBA8 pixels or, for HAP video, as
//! block-compressed texture data uploaded as is (BC1, BC3, BC7, or YCoCg in
//! BC3 converted to RGB by the media shader).

/// Pixel layout of a media texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaFormat {
    /// RGBA8 sRGB pixels.
    #[default]
    Rgba8,
    Bc1,
    Bc3,
    Bc7,
    /// Scaled YCoCg in BC3 (HAP Q).
    YCoCgBc3,
}

impl MediaFormat {
    fn texture_format(self) -> wgpu::TextureFormat {
        match self {
            MediaFormat::Rgba8 => wgpu::TextureFormat::Rgba8UnormSrgb,
            MediaFormat::Bc1 => wgpu::TextureFormat::Bc1RgbaUnormSrgb,
            MediaFormat::Bc3 => wgpu::TextureFormat::Bc3RgbaUnormSrgb,
            MediaFormat::Bc7 => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
            // YCoCg values are not colors: read raw, converted in the shader.
            MediaFormat::YCoCgBc3 => wgpu::TextureFormat::Bc3RgbaUnorm,
        }
    }

    fn compressed(self) -> bool {
        self != MediaFormat::Rgba8
    }

    /// Bytes per row of texels (RGBA) or of 4×4 blocks (BC).
    fn bytes_per_row(self, width: u32) -> u32 {
        match self {
            MediaFormat::Rgba8 => width * 4,
            MediaFormat::Bc1 => width.div_ceil(4) * 8,
            MediaFormat::Bc3 | MediaFormat::Bc7 | MediaFormat::YCoCgBc3 => width.div_ceil(4) * 16,
        }
    }

    /// Whether `device` can sample this format.
    pub fn supported(self, device: &wgpu::Device) -> bool {
        !self.compressed()
            || device
                .features()
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    }
}

/// A decoded frame ready to be drawn, as passed to [`crate::Renderer::render`].
#[derive(Clone, Copy)]
pub struct MediaView<'a> {
    pub view: &'a wgpu::TextureView,
    /// Size of the image.
    pub size: (u32, u32),
    /// Part of the texture holding the image (block-compressed textures are
    /// padded to multiples of 4).
    pub crop: (f32, f32),
    pub ycocg: bool,
}

impl<'a> MediaView<'a> {
    /// A view of a whole RGBA texture.
    pub fn rgba(view: &'a wgpu::TextureView, size: (u32, u32)) -> Self {
        Self {
            view,
            size,
            crop: (1.0, 1.0),
            ycocg: false,
        }
    }
}

/// A texture that frames are uploaded into. It is recreated when the frame
/// size or format changes.
pub struct MediaTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    format: MediaFormat,
}

impl MediaTexture {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        Self::with_format(device, size, MediaFormat::Rgba8)
    }

    pub fn with_format(device: &wgpu::Device, size: (u32, u32), format: MediaFormat) -> Self {
        let size = (size.0.max(1), size.1.max(1));
        // Block-compressed textures must cover whole blocks.
        let extent = if format.compressed() {
            (size.0.next_multiple_of(4), size.1.next_multiple_of(4))
        } else {
            size
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("media"),
            size: wgpu::Extent3d {
                width: extent.0,
                height: extent.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: format.texture_format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            texture,
            view,
            size,
            format,
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
        self.upload_as(device, queue, size, MediaFormat::Rgba8, data)
    }

    /// Uploads `data` in `format`: RGBA8 pixels or 4×4 blocks, rows top to
    /// bottom. Returns `true` when the texture had to be recreated.
    pub fn upload_as(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        format: MediaFormat,
        data: &[u8],
    ) -> bool {
        let recreated = size != self.size || format != self.format;
        if recreated {
            *self = Self::with_format(device, size, format);
        }
        let extent = self.texture.size();
        let rows = if format.compressed() {
            extent.height / 4
        } else {
            extent.height
        };
        let bytes_per_row = format.bytes_per_row(extent.width);
        if data.len() < (bytes_per_row * rows) as usize {
            return recreated;
        }
        queue.write_texture(
            self.texture.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(rows),
            },
            extent,
        );
        recreated
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn format(&self) -> MediaFormat {
        self.format
    }

    pub fn media_view(&self) -> MediaView<'_> {
        let extent = self.texture.size();
        MediaView {
            view: &self.view,
            size: self.size,
            crop: (
                self.size.0 as f32 / extent.width as f32,
                self.size.1 as f32 / extent.height as f32,
            ),
            ycocg: self.format == MediaFormat::YCoCgBc3,
        }
    }
}

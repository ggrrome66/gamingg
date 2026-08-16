//! Device setup.
//!
//! The context can be created with or without a surface. The headless path is
//! not a curiosity — it is how the render pipeline gets tested without a GPU
//! or a display, against a software Vulkan implementation such as lavapipe.

/// Depth format used everywhere. `Depth32Float` is universally supported and
/// avoids the stencil byte we have no use for.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no graphics adapter available: {0}")]
    NoAdapter(#[from] wgpu::RequestAdapterError),
    #[error("could not create a device: {0}")]
    NoDevice(#[from] wgpu::RequestDeviceError),
    #[error("could not create a surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
}

/// Owns the device and queue, plus the surface when there is a window.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// Pick an adapter and open a device, optionally compatible with `surface`.
    async fn create(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface,
            })
            .await?;

        let info = adapter.get_info();
        log::info!(
            "adapter: {} ({:?}, {:?} driver {})",
            info.name,
            info.device_type,
            info.backend,
            info.driver
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("vx device"),
                // Nothing here needs features beyond the baseline yet, and
                // staying at defaults keeps software adapters usable.
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await?;

        Ok(GpuContext {
            instance,
            adapter,
            device,
            queue,
        })
    }

    /// A context with no surface, for tests and offline rendering.
    ///
    /// Reads instance options from the environment, so `WGPU_BACKEND` can pin
    /// a specific backend — which is how tests select a software adapter.
    pub async fn headless() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        Self::create(instance, None).await
    }

    /// Blocking version of [`GpuContext::headless`].
    pub fn headless_blocking() -> Result<Self, GpuError> {
        pollster::block_on(Self::headless())
    }

    /// A context rendering to `window`, returning the configured surface.
    pub async fn for_window(
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<(Self, WindowSurface), GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window)?;
        let context = Self::create(instance, Some(&surface)).await?;

        let config = surface_config(&surface, &context.adapter, width, height);
        surface.configure(&context.device, &config);

        Ok((context, WindowSurface { surface, config }))
    }

    /// A depth texture view sized to the target.
    pub fn create_depth_view(&self, width: u32, height: u32) -> wgpu::TextureView {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }
}

/// Choose a surface configuration, preferring an sRGB format so colour
/// handling matches the shader's assumptions.
fn surface_config(
    surface: &wgpu::Surface<'_>,
    adapter: &wgpu::Adapter,
    width: u32,
    height: u32,
) -> wgpu::SurfaceConfiguration {
    let caps = surface.get_capabilities(adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|format| format.is_srgb())
        .unwrap_or(caps.formats[0]);

    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: width.max(1),
        height: height.max(1),
        present_mode: caps
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::Mailbox)
            .unwrap_or(wgpu::PresentMode::Fifo),
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    }
}

/// A window surface and its current configuration.
///
/// Named to avoid confusion with `wgpu::SurfaceTarget`, which is the handle
/// you pass *in* to create one.
pub struct WindowSurface {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

impl WindowSurface {
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Re-configure after a window resize. Zero sizes are ignored, since a
    /// minimised window reports them and configuring at zero is invalid.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 || (width == self.config.width && height == self.config.height)
        {
            return false;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(device, &self.config);
        true
    }
}

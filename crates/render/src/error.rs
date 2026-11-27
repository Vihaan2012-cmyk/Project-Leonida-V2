use ash::vk;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("the Vulkan loader could not be opened: {0}")]
    Loader(String),
    #[error("Vulkan call failed: {0}")]
    Vulkan(#[from] vk::Result),
    #[error("memory allocation failed: {0}")]
    Allocation(#[from] gpu_allocator::AllocationError),
    #[error("no Vulkan 1.3 GPU with a graphics and compute queue was found")]
    NoDevice,
    #[error("this GPU lacks {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Invalid(String),
    #[error("no memory type fits the resource")]
    NoMemoryType,
    #[error("the bindless {0} array is full")]
    BindlessFull(&'static str),
    #[error("bindless table: {0}")]
    Table(#[from] shader_translate::images::TableError),
    #[error("the queue family cannot present to this surface")]
    NoPresentSupport,
    #[error("the surface was lost")]
    SurfaceLost,
    #[error("window handle: {0}")]
    Window(String),
    #[error("the staging ring holds only unsubmitted allocations; submit and mark them first")]
    StagingFull,
    #[error("a pointer or size is not aligned to {0} bytes")]
    Misaligned(u64),
}

pub type Result<T> = std::result::Result<T, RenderError>;

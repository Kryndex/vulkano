// Copyright (c) 2016 The vulkano developers
// Licensed under the Apache License, Version 2.0
// <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT
// license <LICENSE-MIT or http://opensource.org/licenses/MIT>,
// at your option. All files in the project carrying such
// notice may not be copied, modified, or distributed except
// according to those terms.

use std::error;
use std::fmt;
use std::mem;
use std::ptr;
use std::sync::Arc;
use smallvec::SmallVec;

use check_errors;
use Error;
use OomError;
use VulkanObject;
use vk;

use descriptor::descriptor::DescriptorDesc;
use descriptor::descriptor::ShaderStages;
use descriptor::descriptor_set::UnsafeDescriptorSetLayout;
use descriptor::pipeline_layout::PipelineLayoutDesc;
use descriptor::pipeline_layout::PipelineLayoutDescNames;
use descriptor::pipeline_layout::PipelineLayoutDescPcRange;
use descriptor::pipeline_layout::PipelineLayoutAbstract;
use device::Device;
use device::DeviceOwned;

/// Wrapper around the `PipelineLayout` Vulkan object. Describes to the Vulkan implementation the
/// descriptor sets and push constants available to your shaders 
pub struct PipelineLayout<L> {
    device: Arc<Device>,
    layout: vk::PipelineLayout,
    layouts: SmallVec<[Arc<UnsafeDescriptorSetLayout>; 16]>,
    desc: L,
}

impl<L> PipelineLayout<L> where L: PipelineLayoutDesc {
    /// Creates a new `PipelineLayout`.
    ///
    /// # Panic
    ///
    /// - Panics if one of the layout returned by `provided_set_layout()` belongs to a different
    ///   device than the one passed as parameter.
    #[inline]
    pub fn new(device: Arc<Device>, desc: L)
               -> Result<PipelineLayout<L>, PipelineLayoutCreationError>
    {
        let vk = device.pointers();
        let limits = device.physical_device().limits();

        // Building the list of `UnsafeDescriptorSetLayout` objects.
        let layouts = {
            let mut layouts: SmallVec<[_; 16]> = SmallVec::new();
            for num in 0 .. desc.num_sets() {
                layouts.push(match desc.provided_set_layout(num) {
                    Some(l) => {
                        assert_eq!(l.device().internal_object(), device.internal_object());
                        l
                    },
                    None => {
                        let sets_iter = 0 .. desc.num_bindings_in_set(num).unwrap_or(0);
                        let desc_iter = sets_iter.map(|d| desc.descriptor(num, d));
                        Arc::new(try!(UnsafeDescriptorSetLayout::new(device.clone(), desc_iter)))
                    },
                });
            }
            layouts
        };

        // Grab the list of `vkDescriptorSetLayout` objects from `layouts`.
        let layouts_ids = layouts.iter().map(|l| {
            l.internal_object()
        }).collect::<SmallVec<[_; 16]>>();

        // FIXME: must also check per-descriptor-type limits (eg. max uniform buffer descriptors)

        if layouts_ids.len() > limits.max_bound_descriptor_sets() as usize {
            return Err(PipelineLayoutCreationError::MaxDescriptorSetsLimitExceeded);
        }

        // Builds a list of `vkPushConstantRange` that describe the push constants.
        let push_constants = {
            let mut out: SmallVec<[_; 8]> = SmallVec::new();

            for pc_id in 0 .. desc.num_push_constants_ranges() {
                let PipelineLayoutDescPcRange { offset, size, stages } = {
                    match desc.push_constants_range(pc_id) {
                        Some(o) => o,
                        None => continue,
                    }
                };

                if stages == ShaderStages::none() || size == 0 || (size % 4) != 0 {
                    return Err(PipelineLayoutCreationError::InvalidPushConstant);
                }

                if offset + size > limits.max_push_constants_size() as usize {
                    return Err(PipelineLayoutCreationError::MaxPushConstantsSizeExceeded);
                }

                out.push(vk::PushConstantRange {
                    stageFlags: stages.into(),
                    offset: offset as u32,
                    size: size as u32,
                });
            }

            out
        };

        // Each bit of `stageFlags` must only be present in a single push constants range.
        // We check that with a debug_assert because it's supposed to be enforced by the
        // `PipelineLayoutDesc`.
        debug_assert!({
            let mut stages = 0;
            let mut outcome = true;
            for pc in push_constants.iter() {
                if (stages & pc.stageFlags) != 0 {
                    outcome = false;
                    break;
                }
                stages &= pc.stageFlags;
            }
            outcome
        });

        // FIXME: it is not legal to pass eg. the TESSELLATION_SHADER bit when the device doesn't
        //        have tess shaders enabled

        // Build the final object.
        let layout = unsafe {
            let infos = vk::PipelineLayoutCreateInfo {
                sType: vk::STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
                pNext: ptr::null(),
                flags: 0,   // reserved
                setLayoutCount: layouts_ids.len() as u32,
                pSetLayouts: layouts_ids.as_ptr(),
                pushConstantRangeCount: push_constants.len() as u32,
                pPushConstantRanges: push_constants.as_ptr(),
            };

            let mut output = mem::uninitialized();
            try!(check_errors(vk.CreatePipelineLayout(device.internal_object(), &infos,
                                                      ptr::null(), &mut output)));
            output
        };

        Ok(PipelineLayout {
            device: device.clone(),
            layout: layout,
            layouts: layouts,
            desc: desc,
        })
    }
}

impl<L> PipelineLayout<L> where L: PipelineLayoutDesc {
    /// Returns the description of the pipeline layout.
    #[inline]
    pub fn desc(&self) -> &L {
        &self.desc
    }
}

unsafe impl<D> PipelineLayoutAbstract for PipelineLayout<D> where D: PipelineLayoutDescNames {
    #[inline]
    fn sys(&self) -> PipelineLayoutSys {
        PipelineLayoutSys(&self.layout)
    }

    #[inline]
    fn descriptor_set_layout(&self, index: usize) -> Option<&Arc<UnsafeDescriptorSetLayout>> {
        self.layouts.get(index)
    }
}

unsafe impl<D> PipelineLayoutDesc for PipelineLayout<D> where D: PipelineLayoutDesc {
    #[inline]
    fn num_sets(&self) -> usize {
        self.desc.num_sets()
    }

    #[inline]
    fn num_bindings_in_set(&self, set: usize) -> Option<usize> {
        self.desc.num_bindings_in_set(set)
    }

    #[inline]
    fn descriptor(&self, set: usize, binding: usize) -> Option<DescriptorDesc> {
        self.desc.descriptor(set, binding)
    }

    #[inline]
    fn num_push_constants_ranges(&self) -> usize {
        self.desc.num_push_constants_ranges()
    }

    #[inline]
    fn push_constants_range(&self, num: usize) -> Option<PipelineLayoutDescPcRange> {
        self.desc.push_constants_range(num)
    }
}

unsafe impl<D> PipelineLayoutDescNames for PipelineLayout<D> where D: PipelineLayoutDescNames {
    #[inline]
    fn descriptor_by_name(&self, name: &str) -> Option<(usize, usize)> {
        self.desc.descriptor_by_name(name)
    }
}

unsafe impl<D> DeviceOwned for PipelineLayout<D> {
    #[inline]
    fn device(&self) -> &Arc<Device> {
        &self.device
    }
}

impl<D> fmt::Debug for PipelineLayout<D> where D: fmt::Debug {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt.debug_struct("PipelineLayout")
            .field("raw", &self.layout)
            .field("device", &self.device)
            .field("desc", &self.desc)
            .finish()
    }
}

impl<L> Drop for PipelineLayout<L> {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            let vk = self.device.pointers();
            vk.DestroyPipelineLayout(self.device.internal_object(), self.layout, ptr::null());
        }
    }
}

/// Opaque object that is borrowed from a `PipelineLayout`.
///
/// This object exists so that we can pass it around without having to be generic over the template
/// parameter of the `PipelineLayout`.
#[derive(Copy, Clone)]
pub struct PipelineLayoutSys<'a>(&'a vk::PipelineLayout);

unsafe impl<'a> VulkanObject for PipelineLayoutSys<'a> {
    type Object = vk::PipelineLayout;

    #[inline]
    fn internal_object(&self) -> vk::PipelineLayout {
        *self.0
    }
}

/// Error that can happen when creating an instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineLayoutCreationError {
    /// Not enough memory.
    OomError(OomError),
    /// The maximum number of descriptor sets has been exceeded.
    MaxDescriptorSetsLimitExceeded,
    /// The maximum size of push constants has been exceeded.
    MaxPushConstantsSizeExceeded,
    /// One of the push constants range didn't obey the rules. The list of stages must not be
    /// empty, the size must not be 0, and the size must be a multiple or 4.
    InvalidPushConstant,
}

impl error::Error for PipelineLayoutCreationError {
    #[inline]
    fn description(&self) -> &str {
        match *self {
            PipelineLayoutCreationError::OomError(_) => {
                "not enough memory available"
            },
            PipelineLayoutCreationError::MaxDescriptorSetsLimitExceeded => {
                "the maximum number of descriptor sets has been exceeded"
            },
            PipelineLayoutCreationError::MaxPushConstantsSizeExceeded => {
                "the maximum size of push constants has been exceeded"
            },
            PipelineLayoutCreationError::InvalidPushConstant => {
                "one of the push constants range didn't obey the rules"
            },
        }
    }

    #[inline]
    fn cause(&self) -> Option<&error::Error> {
        match *self {
            PipelineLayoutCreationError::OomError(ref err) => Some(err),
            _ => None
        }
    }
}

impl fmt::Display for PipelineLayoutCreationError {
    #[inline]
    fn fmt(&self, fmt: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(fmt, "{}", error::Error::description(self))
    }
}

impl From<OomError> for PipelineLayoutCreationError {
    #[inline]
    fn from(err: OomError) -> PipelineLayoutCreationError {
        PipelineLayoutCreationError::OomError(err)
    }
}

impl From<Error> for PipelineLayoutCreationError {
    #[inline]
    fn from(err: Error) -> PipelineLayoutCreationError {
        match err {
            err @ Error::OutOfHostMemory => {
                PipelineLayoutCreationError::OomError(OomError::from(err))
            },
            err @ Error::OutOfDeviceMemory => {
                PipelineLayoutCreationError::OomError(OomError::from(err))
            },
            _ => panic!("unexpected error: {:?}", err)
        }
    }
}

/* TODO: restore
#[cfg(test)]
mod tests {
    use std::iter;
    use std::sync::Arc;
    use descriptor::descriptor::ShaderStages;
    use descriptor::descriptor_set::UnsafeDescriptorSetLayout;
    use descriptor::pipeline_layout::sys::PipelineLayout;
    use descriptor::pipeline_layout::sys::PipelineLayoutCreationError;

    #[test]
    fn empty() {
        let (device, _) = gfx_dev_and_queue!();
        let _layout = PipelineLayout::new(&device, iter::empty(), iter::empty()).unwrap();
    }

    #[test]
    #[should_panic]
    fn wrong_device_panic() {
        let (device1, _) = gfx_dev_and_queue!();
        let (device2, _) = gfx_dev_and_queue!();

        let set = match UnsafeDescriptorSetLayout::raw(device1, iter::empty()) {
            Ok(s) => Arc::new(s),
            Err(_) => return
        };

        let _ = PipelineLayout::new(&device2, Some(&set), iter::empty());
    }

    #[test]
    fn invalid_push_constant_stages() {
        let (device, _) = gfx_dev_and_queue!();

        let push_constant = (0, 8, ShaderStages::none());

        match PipelineLayout::new(&device, iter::empty(), Some(push_constant)) {
            Err(PipelineLayoutCreationError::InvalidPushConstant) => (),
            _ => panic!()
        }
    }

    #[test]
    fn invalid_push_constant_size1() {
        let (device, _) = gfx_dev_and_queue!();

        let push_constant = (0, 0, ShaderStages::all_graphics());

        match PipelineLayout::new(&device, iter::empty(), Some(push_constant)) {
            Err(PipelineLayoutCreationError::InvalidPushConstant) => (),
            _ => panic!()
        }
    }

    #[test]
    fn invalid_push_constant_size2() {
        let (device, _) = gfx_dev_and_queue!();

        let push_constant = (0, 11, ShaderStages::all_graphics());

        match PipelineLayout::new(&device, iter::empty(), Some(push_constant)) {
            Err(PipelineLayoutCreationError::InvalidPushConstant) => (),
            _ => panic!()
        }
    }
}*/

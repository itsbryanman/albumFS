use std::ffi::{c_char, c_int, c_ulong, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;

const ERROR_LEN: usize = 256;

type BlockVisitor = unsafe extern "C" fn(
    context: *mut c_void,
    component: u32,
    row: u32,
    column: u32,
    block: *mut i16,
) -> c_int;

unsafe extern "C" {
    fn albumfs_jpeg_read(
        input: *const u8,
        input_len: c_ulong,
        visitor: BlockVisitor,
        context: *mut c_void,
        error_message: *mut c_char,
        error_message_len: usize,
    ) -> c_int;

    fn albumfs_jpeg_write(
        input: *const u8,
        input_len: c_ulong,
        visitor: BlockVisitor,
        context: *mut c_void,
        output: *mut *mut u8,
        output_len: *mut c_ulong,
        error_message: *mut c_char,
        error_message_len: usize,
    ) -> c_int;

    fn albumfs_jpeg_free(pointer: *mut c_void);
}

#[derive(Debug, Clone)]
pub(crate) struct JpegImage {
    pub(crate) components: Vec<Vec<[i16; 64]>>,
}

#[derive(Debug)]
pub(crate) struct JpegFfiError(pub(crate) String);

struct ReadContext {
    components: Vec<Vec<[i16; 64]>>,
}

struct WriteContext<'a> {
    image: &'a JpegImage,
    positions: Vec<usize>,
}

pub(crate) fn read_coefficients(bytes: &[u8]) -> Result<JpegImage, JpegFfiError> {
    ensure_mozjpeg_linked();
    let input_len = c_ulong::try_from(bytes.len())
        .map_err(|_| JpegFfiError("JPEG input is too large".into()))?;
    let mut context = ReadContext {
        components: Vec::new(),
    };
    let mut error = [0u8; ERROR_LEN];
    let status = unsafe {
        albumfs_jpeg_read(
            bytes.as_ptr(),
            input_len,
            read_block,
            (&mut context as *mut ReadContext).cast(),
            error.as_mut_ptr().cast(),
            error.len(),
        )
    };
    check_status(status, &error)?;
    Ok(JpegImage {
        components: context.components,
    })
}

pub(crate) fn write_coefficients(
    original: &[u8],
    image: &JpegImage,
) -> Result<Vec<u8>, JpegFfiError> {
    ensure_mozjpeg_linked();
    let input_len = c_ulong::try_from(original.len())
        .map_err(|_| JpegFfiError("JPEG input is too large".into()))?;
    let mut context = WriteContext {
        image,
        positions: vec![0; image.components.len()],
    };
    let mut output = ptr::null_mut();
    let mut output_len = 0;
    let mut error = [0u8; ERROR_LEN];
    let status = unsafe {
        albumfs_jpeg_write(
            original.as_ptr(),
            input_len,
            write_block,
            (&mut context as *mut WriteContext<'_>).cast(),
            &mut output,
            &mut output_len,
            error.as_mut_ptr().cast(),
            error.len(),
        )
    };
    check_status(status, &error)?;
    if output.is_null() {
        return Err(JpegFfiError("libjpeg returned no output".into()));
    }
    let output_len =
        usize::try_from(output_len).map_err(|_| JpegFfiError("JPEG output is too large".into()))?;
    let bytes = unsafe { slice::from_raw_parts(output, output_len).to_vec() };
    unsafe { albumfs_jpeg_free(output.cast()) };

    if context
        .positions
        .iter()
        .zip(&image.components)
        .any(|(position, component)| *position != component.len())
    {
        return Err(JpegFfiError(
            "coefficient layout changed while writing".into(),
        ));
    }
    Ok(bytes)
}

unsafe extern "C" fn read_block(
    context: *mut c_void,
    component: u32,
    _row: u32,
    _column: u32,
    block: *mut i16,
) -> c_int {
    catch_unwind(AssertUnwindSafe(|| {
        let context = unsafe { &mut *context.cast::<ReadContext>() };
        let component = component as usize;
        if component >= context.components.len() {
            context.components.resize_with(component + 1, Vec::new);
        }
        let source = unsafe { &*block.cast::<[i16; 64]>() };
        context.components[component].push(*source);
    }))
    .map_or(1, |_| 0)
}

unsafe extern "C" fn write_block(
    context: *mut c_void,
    component: u32,
    _row: u32,
    _column: u32,
    block: *mut i16,
) -> c_int {
    catch_unwind(AssertUnwindSafe(|| {
        let context = unsafe { &mut *context.cast::<WriteContext<'_>>() };
        let component = component as usize;
        let Some(blocks) = context.image.components.get(component) else {
            return false;
        };
        let Some(position) = context.positions.get_mut(component) else {
            return false;
        };
        let Some(source) = blocks.get(*position) else {
            return false;
        };
        let target = unsafe { &mut *block.cast::<[i16; 64]>() };
        *target = *source;
        *position += 1;
        true
    }))
    .map_or(1, |accepted| if accepted { 0 } else { 1 })
}

fn check_status(status: c_int, error: &[u8; ERROR_LEN]) -> Result<(), JpegFfiError> {
    if status == 1 {
        return Ok(());
    }
    let end = error
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(error.len());
    let message = String::from_utf8_lossy(&error[..end]);
    if message.is_empty() {
        Err(JpegFfiError("unknown libjpeg error".into()))
    } else {
        Err(JpegFfiError(message.into_owned()))
    }
}

#[inline(never)]
fn ensure_mozjpeg_linked() {
    std::hint::black_box(mozjpeg_sys::jpeg_std_error as *const () as usize);
}

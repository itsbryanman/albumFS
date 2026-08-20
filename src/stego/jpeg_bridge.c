#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <jpeglib.h>

typedef int (*albumfs_block_visitor)(void *context,
                                     uint32_t component,
                                     uint32_t row,
                                     uint32_t column,
                                     JCOEF *block);

struct albumfs_error_mgr {
    struct jpeg_error_mgr base;
    jmp_buf jump;
    char message[JMSG_LENGTH_MAX];
};

struct albumfs_operation {
    struct jpeg_decompress_struct *decompress;
    struct jpeg_compress_struct *compress;
    struct albumfs_error_mgr *error;
    unsigned char *output;
    unsigned long output_len;
    int decompress_created;
    int compress_created;
};

static void albumfs_error_exit(j_common_ptr cinfo) {
    struct albumfs_error_mgr *error = (struct albumfs_error_mgr *)cinfo->err;
    (*cinfo->err->format_message)(cinfo, error->message);
    longjmp(error->jump, 1);
}

static void albumfs_copy_error(char *target,
                               size_t target_len,
                               const char *message) {
    if (target == NULL || target_len == 0) {
        return;
    }
    snprintf(target, target_len, "%s", message);
}

static void albumfs_cleanup(struct albumfs_operation *operation) {
    if (operation == NULL) {
        return;
    }
    if (operation->compress_created) {
        jpeg_destroy_compress(operation->compress);
    }
    if (operation->decompress_created) {
        jpeg_destroy_decompress(operation->decompress);
    }
    free(operation->output);
    free(operation->compress);
    free(operation->decompress);
    free(operation->error);
    free(operation);
}

static int albumfs_visit_jpeg(const unsigned char *input,
                              unsigned long input_len,
                              int writable,
                              albumfs_block_visitor visitor,
                              void *context,
                              unsigned char **output,
                              unsigned long *output_len,
                              char *error_message,
                              size_t error_message_len) {
    struct albumfs_operation *operation = calloc(1, sizeof(*operation));
    if (operation == NULL) {
        albumfs_copy_error(error_message, error_message_len, "out of memory");
        return -1;
    }
    operation->decompress = calloc(1, sizeof(*operation->decompress));
    operation->compress = calloc(1, sizeof(*operation->compress));
    operation->error = calloc(1, sizeof(*operation->error));
    if (operation->decompress == NULL || operation->compress == NULL ||
        operation->error == NULL) {
        albumfs_copy_error(error_message, error_message_len, "out of memory");
        albumfs_cleanup(operation);
        return -1;
    }

    jpeg_std_error(&operation->error->base);
    operation->error->base.error_exit = albumfs_error_exit;
    operation->decompress->err = &operation->error->base;

    if (setjmp(operation->error->jump)) {
        albumfs_copy_error(error_message,
                           error_message_len,
                           operation->error->message);
        albumfs_cleanup(operation);
        return 0;
    }

    jpeg_create_decompress(operation->decompress);
    operation->decompress_created = 1;
    jpeg_mem_src(operation->decompress, input, input_len);
    jpeg_read_header(operation->decompress, TRUE);
    jvirt_barray_ptr *arrays = jpeg_read_coefficients(operation->decompress);
    if (arrays == NULL) {
        albumfs_copy_error(error_message,
                           error_message_len,
                           "libjpeg returned no coefficient arrays");
        albumfs_cleanup(operation);
        return 0;
    }

    for (int component = 0;
         component < operation->decompress->num_components;
         component++) {
        jpeg_component_info *info = &operation->decompress->comp_info[component];
        for (JDIMENSION row = 0; row < info->height_in_blocks; row++) {
            JBLOCKARRAY rows = (*operation->decompress->mem->access_virt_barray)(
                (j_common_ptr)operation->decompress,
                arrays[component],
                row,
                1,
                writable ? TRUE : FALSE);
            for (JDIMENSION column = 0;
                 column < info->width_in_blocks;
                 column++) {
                if (visitor(context,
                            (uint32_t)component,
                            (uint32_t)row,
                            (uint32_t)column,
                            rows[0][column]) != 0) {
                    albumfs_copy_error(error_message,
                                       error_message_len,
                                       "coefficient visitor rejected the image");
                    albumfs_cleanup(operation);
                    return -2;
                }
            }
        }
    }

    if (writable) {
        operation->compress->err = &operation->error->base;
        jpeg_create_compress(operation->compress);
        operation->compress_created = 1;
        jpeg_mem_dest(operation->compress,
                      &operation->output,
                      &operation->output_len);
        jpeg_copy_critical_parameters(operation->decompress,
                                      operation->compress);
        jpeg_write_coefficients(operation->compress, arrays);
        jpeg_finish_compress(operation->compress);
    }
    if (!jpeg_finish_decompress(operation->decompress)) {
        albumfs_copy_error(error_message,
                           error_message_len,
                           "libjpeg suspended while finishing input");
        albumfs_cleanup(operation);
        return 0;
    }

    if (writable) {
        *output = operation->output;
        *output_len = operation->output_len;
        operation->output = NULL;
    }
    albumfs_cleanup(operation);
    return 1;
}

int albumfs_jpeg_read(const unsigned char *input,
                      unsigned long input_len,
                      albumfs_block_visitor visitor,
                      void *context,
                      char *error_message,
                      size_t error_message_len) {
    return albumfs_visit_jpeg(input,
                              input_len,
                              0,
                              visitor,
                              context,
                              NULL,
                              NULL,
                              error_message,
                              error_message_len);
}

int albumfs_jpeg_write(const unsigned char *input,
                       unsigned long input_len,
                       albumfs_block_visitor visitor,
                       void *context,
                       unsigned char **output,
                       unsigned long *output_len,
                       char *error_message,
                       size_t error_message_len) {
    return albumfs_visit_jpeg(input,
                              input_len,
                              1,
                              visitor,
                              context,
                              output,
                              output_len,
                              error_message,
                              error_message_len);
}

void albumfs_jpeg_free(void *pointer) {
    free(pointer);
}

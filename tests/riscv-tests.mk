# Builds the official riscv-tests ISA suites we target (RV64IMAFDC + Zicsr, M/S/U).
# The suites are built without C, as upstream builds them, so they also
# check the 32-bit-only paths; rv64uc adds C. With F and D enabled, the csr
# test checks that FP stores trap while mstatus.FS is off.
#
#   make -f tests/riscv-tests.mk -j$(nproc)
#
# Output goes to build/riscv-tests/.

ROOT    := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/..)
src_dir := $(ROOT)/third_party/riscv-tests/isa
env_dir := $(ROOT)/third_party/riscv-tests/env
OUT     ?= $(ROOT)/build/riscv-tests

include $(src_dir)/rv64ui/Makefrag
include $(src_dir)/rv64um/Makefrag
include $(src_dir)/rv64ua/Makefrag
include $(src_dir)/rv64si/Makefrag
include $(src_dir)/rv64mi/Makefrag
include $(src_dir)/rv64uc/Makefrag
include $(src_dir)/rv64uf/Makefrag
include $(src_dir)/rv64ud/Makefrag

RISCV_GCC ?= riscv64-unknown-elf-gcc
MARCH     := -march=rv64imafd_zicsr_zifencei -mabi=lp64
VMARCH    := $(MARCH)
rv64uc_MARCH  := -march=rv64imafdc_zicsr_zifencei -mabi=lp64
rv64uc_VMARCH := $(rv64uc_MARCH)
OPTS      := -static -mcmodel=medany -fvisibility=hidden -nostdlib -nostartfiles
LIBC_INC  ?= /usr/lib/picolibc/riscv64-unknown-elf/include

define suite
$(addprefix $(OUT)/,$($(1)_p_tests)): $(OUT)/$(1)-p-%: $(src_dir)/$(1)/%.S | $(OUT)
	$(RISCV_GCC) $(or $($(1)_MARCH),$(MARCH)) $(OPTS) -I$(env_dir)/p -I$(src_dir)/macros/scalar -T$(env_dir)/p/link.ld $$< -o $$@
$(addprefix $(OUT)/,$($(1)_v_tests)): $(OUT)/$(1)-v-%: $(src_dir)/$(1)/%.S | $(OUT)
	$(RISCV_GCC) $(or $($(1)_VMARCH),$(VMARCH)) $(OPTS) -DENTROPY=0x$$$$(echo $$(notdir $$@) | md5sum | cut -c 1-7) -std=gnu99 -O2 -isystem $(LIBC_INC) \
	  -I$(env_dir)/v -I$(src_dir)/macros/scalar -T$(env_dir)/v/link.ld $(env_dir)/v/entry.S $(env_dir)/v/*.c $$< -o $$@
ALL += $(addprefix $(OUT)/,$($(1)_p_tests) $($(1)_v_tests))
endef

$(foreach s,rv64ui rv64um rv64ua rv64si rv64mi rv64uc rv64uf rv64ud,$(eval $(call suite,$(s))))

all: $(ALL)
$(OUT):
	mkdir -p $@
.PHONY: all
.DEFAULT_GOAL := all

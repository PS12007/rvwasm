# Builds the official riscv-tests ISA suites we target (RV64IMAC + Zicsr, M/S/U).
# The base suites are built with -march=rv64ima, as upstream builds them
# without C, so they also check the 32-bit-only paths; rv64uc adds C.
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

RISCV_GCC ?= riscv64-unknown-elf-gcc
MARCH     := -march=rv64ima_zicsr_zifencei -mabi=lp64
# The -v env names one FP opcode as data (to detect FP tests); F is enabled
# for the assembler only in effect: the C code is integer-only.
VMARCH    := -march=rv64imaf_zicsr_zifencei -mabi=lp64
rv64uc_MARCH  := -march=rv64imac_zicsr_zifencei -mabi=lp64
rv64uc_VMARCH := -march=rv64imafc_zicsr_zifencei -mabi=lp64
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

$(foreach s,rv64ui rv64um rv64ua rv64si rv64mi rv64uc,$(eval $(call suite,$(s))))

all: $(ALL)
$(OUT):
	mkdir -p $@
.PHONY: all
.DEFAULT_GOAL := all

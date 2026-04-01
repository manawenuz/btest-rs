# OpenWrt package Makefile for btest-rs
#
# To build:
#   1. Clone the OpenWrt SDK for your target
#   2. Copy this directory to package/btest-rs/ in the SDK
#   3. Run: make package/btest-rs/compile V=s
#
# Or use the pre-built binary approach (see build-ipk.sh)

include $(TOPDIR)/rules.mk

PKG_NAME:=btest-rs
PKG_VERSION:=0.6.0
PKG_RELEASE:=1

PKG_SOURCE:=$(PKG_NAME)-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://github.com/manawenuz/btest-rs/archive/refs/tags/v$(PKG_VERSION).tar.gz
PKG_HASH:=skip

PKG_BUILD_DEPENDS:=rust/host
PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_VERSION)

include $(INCLUDE_DIR)/package.mk

define Package/btest-rs
  SECTION:=net
  CATEGORY:=Network
  TITLE:=MikroTik Bandwidth Test server and client
  URL:=https://github.com/manawenuz/btest-rs
  DEPENDS:=
  PKGARCH:=$(ARCH)
endef

define Package/btest-rs/description
  A Rust reimplementation of the MikroTik Bandwidth Test (btest) protocol.
  Supports TCP/UDP, IPv4/IPv6, EC-SRP5 and MD5 authentication,
  multi-connection, syslog, CSV output, and CPU monitoring.
endef

define Build/Compile
	cd $(PKG_BUILD_DIR) && \
	CARGO_TARGET_DIR=$(PKG_BUILD_DIR)/target \
	cargo build --release --target $(RUSTC_TARGET)
endef

define Package/btest-rs/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_BUILD_DIR)/target/$(RUSTC_TARGET)/release/btest $(1)/usr/bin/btest

	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) ./files/btest.init $(1)/etc/init.d/btest

	$(INSTALL_DIR) $(1)/etc/config
	$(INSTALL_CONF) ./files/btest.config $(1)/etc/config/btest
endef

$(eval $(call BuildPackage,btest-rs))

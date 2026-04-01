Name:           btest-rs
Version:        0.6.0
Release:        1%{?dist}
Summary:        MikroTik Bandwidth Test (btest) server and client with EC-SRP5 auth

License:        MIT AND Apache-2.0
URL:            https://github.com/manawenuz/btest-rs
Source0:        https://github.com/manawenuz/btest-rs/archive/refs/tags/v%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
ExclusiveArch:  x86_64 aarch64

%description
A Rust reimplementation of the MikroTik Bandwidth Test (btest) protocol,
providing both server and client functionality with EC-SRP5 authentication.

%prep
%autosetup -n %{name}-%{version}

%build
export CARGO_TARGET_DIR=target
cargo build --release

%install
install -Dm755 target/release/btest %{buildroot}%{_bindir}/btest
install -Dm644 docs/man/btest.1 %{buildroot}%{_mandir}/man1/btest.1
install -Dm644 LICENSE %{buildroot}%{_datadir}/licenses/%{name}/LICENSE

# systemd service unit
install -d %{buildroot}%{_unitdir}
cat > %{buildroot}%{_unitdir}/btest.service << 'EOF'
[Unit]
Description=MikroTik Bandwidth Test Server (btest-rs)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/btest -s
Restart=always
RestartSec=5
DynamicUser=yes
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF

%files
%license LICENSE
%{_bindir}/btest
%{_mandir}/man1/btest.1*
%{_unitdir}/btest.service

%post
%systemd_post btest.service

%preun
%systemd_preun btest.service

%postun
%systemd_postun_with_restart btest.service

%changelog
* Mon Mar 30 2026 Siavash Sameni <manwe@manko.yoga> - 0.6.0-1
- Initial RPM package

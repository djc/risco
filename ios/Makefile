# Builds, signs and installs the Risco iPhone app with Xcode's command line tools, without
# an Xcode project
#
# Run `make help` to list the targets. Settings for signing and for installing on a device go in
# config.mk (see config.mk.example), which is not checked in.

-include config.mk

BUNDLE_ID     ?= com.example.risco
VERSION       ?= 0.1.0
BUILD_NUMBER  ?= 1
MIN_IOS       ?= 17.0
# The name or SHA-1 hash of the signing identity (see `make identities`); by default, the one in
# the keychain that the provisioning profile allows
SIGN_IDENTITY ?=
# The development provisioning profile from the developer portal
PROFILE       ?=
# The device to install on (see `make devices`)
DEVICE        ?=
# The simulator to run in (see `make simulators`), or the one that is running
SIMULATOR     ?= booted

APP     := Risco
BUILD   := build
SOURCES := $(wildcard Sources/*.swift)
CONFIG  := Makefile $(wildcard config.mk)

SIM_APP    := $(BUILD)/iphonesimulator/$(APP).app
DEVICE_APP := $(BUILD)/iphoneos/$(APP).app

# Settings for each SDK, looked up by its name
TARGET_iphoneos          := arm64-apple-ios$(MIN_IOS)
TARGET_iphonesimulator   := arm64-apple-ios$(MIN_IOS)-simulator
PLATFORM_iphoneos        := iPhoneOS
PLATFORM_iphonesimulator := iPhoneSimulator

SWIFT_FLAGS := -swift-version 6 -parse-as-library -module-name $(APP) -O

# The app that shows simulators: DeviceHub in recent versions of Xcode, Simulator in older ones.
# Opened by path, because Launch Services may not know about apps inside Xcode.
DEVELOPER_DIR := $(shell xcode-select -p)
SIMULATOR_VIEWER := $(firstword $(wildcard $(DEVELOPER_DIR)/../Applications/DeviceHub.app \
	$(DEVELOPER_DIR)/Applications/Simulator.app))

# Fail with a hint if a setting is missing: $(call require,NAME,hint)
require = $(if $($(1)),,$(error $(1) is not set; $(2)))

.DEFAULT_GOAL := help
.DELETE_ON_ERROR:
# Keep intermediate files, such as each platform's assets.plist
.SECONDARY:
.PHONY: help sim run-sim device install run lsp clean simulators devices identities csr \
	import-cert

## Show this help
help:
	@awk '/^## / { doc = substr($$0, 4) } /^[a-z-]+:/ && doc { sub(/:.*/, ""); \
		printf "  %-12s %s\n", $$0, doc; doc = "" }' $(firstword $(MAKEFILE_LIST))

## Build the app for the simulator
sim: $(SIM_APP)/_CodeSignature/CodeResources

## Build the app and run it in the simulator
run-sim: sim
	xcrun simctl bootstatus '$(SIMULATOR)' -b
	$(if $(SIMULATOR_VIEWER),open '$(SIMULATOR_VIEWER)',@echo "No app found to show the simulator")
	xcrun simctl install '$(SIMULATOR)' $(SIM_APP)
	xcrun simctl launch --terminate-running-process '$(SIMULATOR)' $(BUNDLE_ID)

## Build and sign the app for a device
device: $(DEVICE_APP)/_CodeSignature/CodeResources

## Install the app on the device
install: device
	$(call require,DEVICE,set it to a name or identifier from `make devices`)
	xcrun devicectl device install app --device '$(DEVICE)' $(DEVICE_APP)

## Install the app on the device and launch it
run: install
	xcrun devicectl device process launch --terminate-existing --device '$(DEVICE)' $(BUNDLE_ID)

# The executable
$(BUILD)/%/$(APP).app/$(APP): $(SOURCES) $(CONFIG)
	@mkdir -p $(@D)
	xcrun --sdk $* swiftc $(SWIFT_FLAGS) -target $(TARGET_$*) -o $@ $(SOURCES)

# The app icon is drawn by a script, rather than checked in as an image
$(BUILD)/Assets.xcassets/Contents.json: Tools/AppIcon.swift
	xcrun --sdk macosx swift $< $(@D)

# The compiled asset catalog, and the Info.plist keys for it
$(BUILD)/%/assets.plist: $(BUILD)/Assets.xcassets/Contents.json $(CONFIG)
	@mkdir -p $(BUILD)/$*/$(APP).app
	xcrun actool $(<D) --compile $(BUILD)/$*/$(APP).app --platform $* \
		--minimum-deployment-target $(MIN_IOS) --target-device iphone --app-icon AppIcon \
		--output-partial-info-plist $@ --output-format human-readable-text --errors --warnings

$(BUILD)/%/$(APP).app/Info.plist: Info.plist $(BUILD)/%/assets.plist $(CONFIG)
	plutil -convert xml1 -o $@ Info.plist
	/usr/libexec/PlistBuddy -c 'Merge $(BUILD)/$*/assets.plist' $@
	plutil -replace CFBundleIdentifier -string '$(BUNDLE_ID)' $@
	plutil -replace CFBundleShortVersionString -string '$(VERSION)' $@
	plutil -replace CFBundleVersion -string '$(BUILD_NUMBER)' $@
	plutil -replace MinimumOSVersion -string '$(MIN_IOS)' $@
	plutil -replace CFBundleSupportedPlatforms -json '["$(PLATFORM_$*)"]' $@
	plutil -replace DTPlatformName -string '$*' $@
	plutil -replace DTSDKName -string "$*$$(xcrun --sdk $* --show-sdk-version)" $@
	plutil -convert binary1 $@

# Simulator apps only need an ad-hoc signature
$(SIM_APP)/_CodeSignature/CodeResources: $(SIM_APP)/$(APP) $(SIM_APP)/Info.plist
	codesign --force --sign - --timestamp=none $(SIM_APP)

$(DEVICE_APP)/_CodeSignature/CodeResources: $(DEVICE_APP)/$(APP) $(DEVICE_APP)/Info.plist \
		$(DEVICE_APP)/embedded.mobileprovision $(BUILD)/entitlements.plist $(BUILD)/identity
	codesign --force --sign "$$(cat $(BUILD)/identity)" \
		--entitlements $(BUILD)/entitlements.plist --timestamp=none $(DEVICE_APP)

PROFILE_HINT := set it to the development provisioning profile (see README.md)

$(DEVICE_APP)/embedded.mobileprovision: $(PROFILE)
	$(call require,PROFILE,$(PROFILE_HINT))
	@mkdir -p $(@D)
	cp $(PROFILE) $@

# The profile's contents, after checking its signature
$(BUILD)/profile.plist: $(PROFILE)
	$(call require,PROFILE,$(PROFILE_HINT))
	@mkdir -p $(@D)
	security cms -D -i $(PROFILE) -o $@

# The signing identity: SIGN_IDENTITY if set, or else the first identity in the keychain whose
# certificate the profile allows. Certificates are named "Apple Development" or, for the older
# "iOS App Development" type, "iPhone Developer", so matching by hash avoids guessing the name.
$(BUILD)/identity: $(BUILD)/profile.plist $(CONFIG)
	@identity='$(SIGN_IDENTITY)'; \
	valid=$$(security find-identity -v -p codesigning); \
	if [ -z "$$identity" ]; then \
		count=$$(plutil -extract DeveloperCertificates raw -o - $<); \
		for i in $$(seq 0 $$((count - 1))); do \
			hash=$$(plutil -extract DeveloperCertificates.$$i raw -o - $< | base64 -D \
				| shasum -a 1 | cut -c1-40 | tr a-f A-F); \
			if echo "$$valid" | grep -qF "$$hash"; then identity=$$hash; break; fi; \
		done; \
	fi; \
	if [ -z "$$identity" ]; then \
		echo "None of the provisioning profile's certificates is in the keychain;" \
			"see \`make identities\`, or set SIGN_IDENTITY"; \
		exit 1; \
	fi; \
	name=$$(echo "$$valid" | grep -m1 -F "$$identity" | cut -d'"' -f2); \
	echo "Signing with $${name:-$$identity}"; \
	echo "$$identity" > $@

# The entitlements the profile grants, for this app's bundle identifier
$(BUILD)/entitlements.plist: $(BUILD)/profile.plist $(CONFIG)
	plutil -extract Entitlements xml1 -o $@ $<
	plutil -replace application-identifier \
		-string "$$(plutil -extract TeamIdentifier.0 raw $<).$(BUNDLE_ID)" $@

comma := ,
LSP_ARGS = "swiftc"$(foreach arg,$(SWIFT_FLAGS) -target $(TARGET_iphonesimulator) \
	-sdk $(shell xcrun --sdk iphonesimulator --show-sdk-path) \
	$(addprefix $(CURDIR)/,$(SOURCES)),$(comma) "$(arg)")

## Write compile_commands.json, which tells SourceKit-LSP how to build for iOS
lsp: compile_commands.json

compile_commands.json: $(SOURCES) $(CONFIG)
	@sep='['; for file in $(SOURCES); do \
		printf '%s\n  {"directory": "%s", "file": "%s", "arguments": [%s]}' \
			"$$sep" '$(CURDIR)' '$(CURDIR)/'"$$file" '$(LSP_ARGS)'; \
		sep=','; \
	done > $@; printf '\n]\n' >> $@
	@echo "Wrote $@"

## Remove build output
clean:
	rm -rf $(BUILD) compile_commands.json

## List the available simulators
simulators:
	xcrun simctl list devices available

## List connected devices, with the UDID needed to register them on the developer portal
devices:
	xcrun devicectl list devices --columns UDID

## List code signing identities
identities:
	security find-identity -v -p codesigning

SIGNING := signing
# LibreSSL, which writes PKCS #12 files that `security import` can read
OPENSSL := /usr/bin/openssl

## Create a private key and signing request for an Apple Development certificate
csr: $(SIGNING)/development.csr
	@echo "Upload $< as a new Apple Development certificate at"
	@echo "https://developer.apple.com/account/resources/certificates/add, then run"
	@echo "make import-cert CERT=<the downloaded .cer file>"

$(SIGNING)/development.key:
	@mkdir -p $(@D)
	umask 077 && $(OPENSSL) genrsa -out $@ 2048

$(SIGNING)/development.csr: $(SIGNING)/development.key
	$(OPENSSL) req -new -key $< -subj '/CN=$(APP) Development' -out $@

# Apple Development certificates are issued by the WWDR G3 intermediate, which may not be in the
# keychain yet; importing it again fails harmlessly
WWDR_URL := https://www.apple.com/certificateauthority/AppleWWDRCAG3.cer

## Import the certificate from the developer portal into the keychain: CERT=<file>
import-cert: $(SIGNING)/development.key
	$(call require,CERT,set it to the .cer file downloaded from the developer portal)
	$(OPENSSL) x509 -inform DER -in $(CERT) -out $(SIGNING)/development.pem
	umask 077 && $(OPENSSL) pkcs12 -export -inkey $< -in $(SIGNING)/development.pem \
		-out $(SIGNING)/development.p12 -passout pass:import
	security import $(SIGNING)/development.p12 -P import -T /usr/bin/codesign
	rm $(SIGNING)/development.p12
	curl -fsS -o $(SIGNING)/AppleWWDRCAG3.cer $(WWDR_URL)
	-security import $(SIGNING)/AppleWWDRCAG3.cer
	security find-identity -v -p codesigning

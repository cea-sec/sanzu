
DOCKER_OPT=-v ${PWD}/../:/SRC --env CARGO_HOME=/SRC/target/.cargo
CARGO_OPT="--release"
DOCKER_USER=`id -u`:`id -g`
SANZU_VERSION:=$(shell grep "^version" ../sanzu/Cargo.toml  | head -1 | cut -d'"' -f2)

debian:
	docker build -t sanzu-builder:debian -f Dockerfile-debian .
	docker run -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:debian
	docker run -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu --variant=client" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:debian
	docker run --rm -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu_broker" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:debian

alpine:
	docker build -t sanzu-builder:alpine -f Dockerfile-alpine .
	docker run -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:alpine
	docker run --rm -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu-broker" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:alpine

almalinux8:
	docker build -t sanzu-builder:almalinux8 -f Dockerfile-almalinux8 .
	docker run -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:almalinux8
	docker run --rm -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu-broker" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:almalinux8

almalinux9:
	docker build -t sanzu-builder:almalinux9 -f Dockerfile-almalinux9 .
	docker run -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:almalinux9
	docker run --rm -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu-broker" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:almalinux9

windows:
	docker build -t sanzu-builder:windows -f Dockerfile-windows .
	docker run --rm -t $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGO_OPT="-p sanzu --release" sanzu-builder:windows
	docker run --rm -t $(DOCKER_OPT) -u ${DOCKER_USER} sanzu-builder:windows bash -c \
		"cd /usr/x86_64-w64-mingw32/lib/ \
		; cp ../bin/libopus-0.dll swscale-*.dll postproc-*.dll \
		     avutil-*.dll avdevice-*.dll avcodec-*.dll \
		     swresample-*.dll ../bin/libssp-*.dll \
		 /SRC/target/x86_64-pc-windows-gnu/release/"
	mkdir ../target/windows
	cd ../target/x86_64-pc-windows-gnu/release ; 7z a ../../windows/sanzu-$(SANZU_VERSION)-x86_64-pc-windows-gnu.zip sanzu_client.exe sanzu_server.exe *.dll

clean:
	rm -fr ../target/

mrproper: clean
	docker image rm sanzu-builder:debian
	docker image rm sanzu-builder:alpine
	docker image rm sanzu-builder:almalinux8
	docker image rm sanzu-builder:almalinux9
	docker image rm sanzu-builder:windows


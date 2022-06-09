
DOCKER_OPT=-v ${PWD}/../:/SRC --env CARGO_HOME=/SRC/target/.cargo
CARGO_OPT="--release"
DOCKER_USER=`id -u`:`id -g`
SANZU_VERSION:=$(shell grep "^version" ../sanzu/Cargo.toml  | head -1 | cut -d'"' -f2)

debian:
	docker build -t sanzu-builder:debian -f Dockerfile-debian .
	docker run -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:debian
	docker run -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu --variant=client" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:debian
	docker run --rm -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu_broker" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:debian

almalinux:
	docker build -t sanzu-builder:almalinux -f Dockerfile-almalinux .
	docker run -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:almalinux
	docker run --rm -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu-broker" \
		--env CARGO_OPT=$(CARGO_OPT) sanzu-builder:almalinux

windows:
	docker build -t sanzu-builder:windows -f Dockerfile-windows .
	docker run --rm -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGO_OPT="-p sanzu --no-default-features --release" sanzu-builder:windows
	docker run --rm -it $(DOCKER_OPT) -u ${DOCKER_USER} sanzu-builder:windows bash -c \
		"cd /usr/x86_64-w64-mingw32/lib/ \
                ; cp ../bin/libopus-0.dll swscale-*.dll postproc-*.dll \
		     avutil-*.dll avdevice-*.dll avcodec-*.dll \
		     swresample-*.dll ../bin/libssp-*.dll \
		 /SRC/target/x86_64-pc-windows-gnu/release/"
	cd ../target/x86_64-pc-windows-gnu/release ; 7z a sanzu-$(SANZU_VERSION)-x86_64-pc-windows-gnu.zip *.exe *.dll

clean:
	rm -fr ../target/

mrproper: clean
	docker image rm sanzu-builder:debian
	docker image rm sanzu-builder:almalinux
	docker image rm sanzu-builder:windows


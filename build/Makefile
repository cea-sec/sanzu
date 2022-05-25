
DOCKER_OPT=-v ${PWD}/../:/SRC --env CARGO_HOME=/SRC/target/.cargo
CARGO_OPT=""
DOCKER_USER=`id -u`:`id -g`

debian:
	docker build -t sanzu-builder:debian -f Dockerfile-debian .
	docker run -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu" sanzu-builder:debian 
	docker run -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu --variant=client" sanzu-builder:debian 
	docker run --rm -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGODEB_OPT="-p sanzu_broker" sanzu-builder:debian 

almalinux:
	docker build -t sanzu-builder:almalinux -f Dockerfile-almalinux .
	docker run -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu" sanzu-builder:almalinux 
	docker run --rm -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGORPM_OPT="-p sanzu-broker" sanzu-builder:almalinux 

windows:
	docker build -t sanzu-builder:windows -f Dockerfile-windows .
	docker run --rm -it $(DOCKER_OPT) -u ${DOCKER_USER} --env CARGO_OPT="-p sanzu --no-default-features" sanzu-builder:windows

clean:
	rm -fr ../target/

mrproper: clean
	docker image rm sanzu-builder:debian
	docker image rm sanzu-builder:almalinux
	docker image rm sanzu-builder:windows


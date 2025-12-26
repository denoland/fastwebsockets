AUTOBAHN_TESTSUITE_DOCKER := crossbario/autobahn-testsuite:latest@sha256:519915fb568b04c9383f70a1c405ae3ff44ab9e35835b085239c258b6fac3074

build-server:
	sudo cargo build --release --example echo_server --features "upgrade"

run-server: build-server
	echo ${PWD}
	docker run -d  \
		--name fuzzingserver \
		-u `id -u`:`id -g` \
		-v ${PWD}/fuzzingclient.json:/fuzzingclient.json:ro \
		-v ${PWD}/reports:/reports \
		-p 9001:9001 \
		--net=host \
		--rm \
		$(AUTOBAHN_TESTSUITE_DOCKER) \
	  wstest -m fuzzingclient -s fuzzingclient.json
	../target/release/examples/echo_server

build-client:
	sudo cargo build --release --example autobahn_client --features "upgrade"

run-client: build-client
	echo ${PWD}
	docker run -d  \
		--name fuzzingserver \
		-u `id -u`:`id -g` \
		-v ${PWD}/fuzzingserver.json:/fuzzingserver.json:ro \
		-v ${PWD}/reports:/reports \
		-p 9001:9001 \
		--rm \
		$(AUTOBAHN_TESTSUITE_DOCKER) \
	  wstest -m fuzzingserver -s fuzzingserver.json
	sleep 5
	../target/release/examples/autobahn_client

.PHONY: build-server run-server build-client run-client

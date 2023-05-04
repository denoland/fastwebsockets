PATH_TO_USOCKETS=../../uWebSockets/uSockets/src

# brew link --force openssl
# /opt/homebrew/opt/openssl@3/
OPENSSL_FLAGS=-L/opt/homebrew/opt/openssl@3/lib -I/opt/homebrew/opt/openssl@3/include

OTHER_LIBS=`ls ../../uWebSockets/benchmarks/*.o | grep -E "bsd|context|loop|quic|socket|udp"`
default:
	clang -flto -O3 -DLIBUS_USE_OPENSSL $(OPENSSL_FLAGS) -I$(PATH_TO_USOCKETS) $(PATH_TO_USOCKETS)/eventing/*.c $(PATH_TO_USOCKETS)/crypto/*.c load_test.c -c
	clang++ -flto -O3 -DLIBUS_USE_OPENSSL $(OPENSSL_FLAGS) -I$(PATH_TO_USOCKETS) $(PATH_TO_USOCKETS)/crypto/*.cpp -c -std=c++17

	clang++ -flto -O3 -DLIBUS_USE_OPENSSL $(OPENSSL_FLAGS) `ls *.o` $(OTHER_LIBS) -lssl -lcrypto -o load_test

#ifndef NIVORA_TURBO_INSTANCE_MANIFEST_HPP
#define NIVORA_TURBO_INSTANCE_MANIFEST_HPP

#include <string>
#include <vector>

namespace nivora {

// принимает argv без команды, печатает TSV и возвращает код завершения
int run_instance_manifest(const std::vector<std::string> &args);

}

#endif // closes NIVORA_TURBO_INSTANCE_MANIFEST_HPP

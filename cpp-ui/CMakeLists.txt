cmake_minimum_required(VERSION 3.21)
project(nivora-launcher-gui LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

find_package(Qt6 REQUIRED COMPONENTS Widgets Svg)

add_executable(nivora-launcher-gui main.cpp)
target_link_libraries(nivora-launcher-gui PRIVATE Qt6::Widgets Qt6::Svg)
target_link_options(nivora-launcher-gui PRIVATE "-pie")

install(TARGETS nivora-launcher-gui RUNTIME DESTINATION bin)
install(DIRECTORY assets/ DESTINATION share/nivora-launcher)

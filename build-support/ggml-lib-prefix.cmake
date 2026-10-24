# Injected into whisper.cpp's CMake build (CMAKE_PROJECT_INCLUDE, see .cargo/config.toml).
# ggml clears the `lib` prefix on Windows (ggml.a); rustc on windows-gnu links `static=ggml` as libggml.a.
# Once the ggml directory is processed, its library targets get the prefix back.

function(omega_ggml_lib_prefix)
    foreach(target ggml ggml-base ggml-cpu ggml-vulkan)
        if(TARGET ${target})
            set_target_properties(${target} PROPERTIES PREFIX "lib")
        endif()
    endforeach()
endfunction()

if(PROJECT_NAME STREQUAL "ggml")
    cmake_language(DEFER CALL omega_ggml_lib_prefix)
endif()

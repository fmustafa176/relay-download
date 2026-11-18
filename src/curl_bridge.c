#include <curl/curl.h>
#include <stdio.h>
#include <string.h>

int relay_curl_init(void) {
    return curl_global_init(CURL_GLOBAL_DEFAULT);
}

int relay_curl(const char *url, const char *range, const char *validator,
               const char *headers, const char *segment, char *effective,
               size_t effective_capacity, int (*progress)(void *, curl_off_t,
               curl_off_t, curl_off_t, curl_off_t), void *context) {
    CURL *curl = curl_easy_init();
    FILE *header_file = fopen(headers, "wb");
    FILE *segment_file = fopen(segment, "wb");
    struct curl_slist *request = curl_slist_append(NULL, "Accept-Encoding: identity");
    CURLcode result = CURLE_FAILED_INIT;

    if (!curl || !header_file || !segment_file || !request)
        goto cleanup;
    setvbuf(header_file, NULL, _IONBF, 0);
    if (range) {
        struct curl_slist *extended = curl_slist_append(request, validator);
        if (!extended)
            goto cleanup;
        request = extended;
        curl_easy_setopt(curl, CURLOPT_RANGE, range);
    }
    curl_easy_setopt(curl, CURLOPT_URL, url);
    curl_easy_setopt(curl, CURLOPT_PROTOCOLS_STR, "http,https");
    curl_easy_setopt(curl, CURLOPT_REDIR_PROTOCOLS_STR, "http,https");
    curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(curl, CURLOPT_MAXREDIRS, 20L);
    curl_easy_setopt(curl, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(curl, CURLOPT_CONNECTTIMEOUT, 10L);
    curl_easy_setopt(curl, CURLOPT_LOW_SPEED_TIME, 15L);
    curl_easy_setopt(curl, CURLOPT_LOW_SPEED_LIMIT, 1L);
    curl_easy_setopt(curl, CURLOPT_HTTPHEADER, request);
    curl_easy_setopt(curl, CURLOPT_HEADERDATA, header_file);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, segment_file);
    curl_easy_setopt(curl, CURLOPT_NOPROGRESS, 0L);
    curl_easy_setopt(curl, CURLOPT_XFERINFOFUNCTION, progress);
    curl_easy_setopt(curl, CURLOPT_XFERINFODATA, context);
    result = curl_easy_perform(curl);
    char *final_url = NULL;
    if (!curl_easy_getinfo(curl, CURLINFO_EFFECTIVE_URL, &final_url) && final_url &&
        strlen(final_url) < effective_capacity)
        strcpy(effective, final_url);

cleanup:
    curl_slist_free_all(request);
    if (curl) curl_easy_cleanup(curl);
    if (header_file) fclose(header_file);
    if (segment_file && fclose(segment_file) && !result) result = CURLE_WRITE_ERROR;
    return result;
}

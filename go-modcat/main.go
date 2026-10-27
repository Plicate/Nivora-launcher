// json CLI для поиска проектов Modrinth, совместимости версий и обновлений
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	appVersion   = "1.1.0"
	userAgent    = "Nivora Launcher/" + appVersion
	httpTimeout  = 30 * time.Second
	maxBatchJobs = 8
)

var projectDirectories = map[string]bool{
	"mod": true, "resourcepack": true, "shader": true, "datapack": true,
}

var searchOrderings = map[string]bool{
	"relevance": true, "downloads": true, "newest": true, "updated": true,
}

type checkEntry struct {
	ProjectID string `json:"project_id"`
	VersionID string `json:"version_id"`
}

type request struct {
	Cmd         string       `json:"cmd"`
	Query       string       `json:"query"`
	GameVersion string       `json:"game_version"`
	Loader      string       `json:"loader"`
	Limit       int          `json:"limit"`
	ProjectType string       `json:"project_type"`
	SortBy      string       `json:"sort_by"`
	Offset      int          `json:"offset"`
	ProjectID   string       `json:"project_id"`
	APIURL      string       `json:"api_url"`
	Workers     int          `json:"workers"`
	Projects    []checkEntry `json:"projects"`
}

func fail(message string) {
	_ = json.NewEncoder(os.Stdout).Encode(map[string]any{"ok": false, "error": message})
	os.Exit(0)
}

func main() {
	raw, err := io.ReadAll(io.LimitReader(os.Stdin, 1<<20))
	if err != nil {
		fail("Не удалось прочитать запрос nivora-modcat")
	}
	var req request
	if err := json.Unmarshal(raw, &req); err != nil {
		fail("Некорректный JSON-запрос nivora-modcat")
	}
	var data any
	switch req.Cmd {
	case "search":
		data, err = cmdSearch(req)
	case "details":
		data, err = cmdDetails(req)
	case "versions":
		data, err = cmdVersions(req)
	case "check":
		data, err = cmdCheck(req)
	default:
		fail(fmt.Sprintf("Неизвестная команда nivora-modcat: %s", req.Cmd))
	}
	if err != nil {
		fail(err.Error())
	}
	_ = json.NewEncoder(os.Stdout).Encode(map[string]any{"ok": true, "data": data})
}

func httpClient() *http.Client {
	return &http.Client{Timeout: httpTimeout}
}

func getJSON(client *http.Client, target string) (any, error) {
	request, err := http.NewRequest(http.MethodGet, target, nil)
	if err != nil {
		return nil, fmt.Errorf("Некорректный URL запроса к каталогу")
	}
	request.Header.Set("User-Agent", userAgent)
	request.Header.Set("Accept", "application/json")
	response, err := client.Do(request)
	if err != nil {
		return nil, fmt.Errorf("Не удалось связаться с каталогом Modrinth: %s", requestURLHost(target))
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, 16<<20))
	if err != nil {
		return nil, fmt.Errorf("Каталог Modrinth вернул некорректный ответ")
	}
	if response.StatusCode >= 400 {
		return nil, fmt.Errorf("Каталог Modrinth ответил ошибкой %d", response.StatusCode)
	}
	var value any
	if err := json.Unmarshal(body, &value); err != nil {
		return nil, fmt.Errorf("Каталог Modrinth вернул некорректный JSON")
	}
	return value, nil
}

func requestURLHost(target string) string {
	parsed, err := url.Parse(target)
	if err != nil {
		return target
	}
	return parsed.Host
}

func baseURL(raw string) string {
	value := strings.TrimSpace(raw)
	if value == "" {
		return "https://api.modrinth.com/v2"
	}
	return strings.TrimRight(value, "/")
}

func validateProjectType(projectType string) error {
	if !projectDirectories[projectType] {
		return fmt.Errorf("Неизвестный тип Modrinth-проекта: %s", projectType)
	}
	return nil
}

func validateProjectID(value string) (string, error) {
	projectID := strings.TrimSpace(value)
	if projectID == "" || len(projectID) > 200 || projectID == "." || projectID == ".." {
		return "", fmt.Errorf("Некорректный project ID ресурса")
	}
	for _, character := range projectID {
		if character < 32 || character == 127 {
			return "", fmt.Errorf("Некорректный project ID ресурса")
		}
	}
	return projectID, nil
}

func asObject(value any) map[string]any {
	object, _ := value.(map[string]any)
	return object
}

func fieldString(object map[string]any, key string) string {
	if object == nil {
		return ""
	}
	text, _ := object[key].(string)
	return strings.TrimSpace(text)
}

// пустой project_type пропускает проекты любого типа
func matchesProjectType(value map[string]any, expected string) bool {
	actual := fieldString(value, "project_type")
	return actual == "" || actual == expected
}

func stringList(version map[string]any, key string) []string {
	raw, _ := version[key].([]any)
	list := make([]string, 0, len(raw))
	for _, item := range raw {
		text, _ := item.(string)
		list = append(list, text)
	}
	return list
}

func versionIsCompatible(version map[string]any, gameVersion, loader, projectType string) bool {
	if gameVersion != "" {
		if _, ok := version["game_versions"]; ok {
			found := false
			for _, item := range stringList(version, "game_versions") {
				if item == gameVersion {
					found = true
					break
				}
			}
			if !found {
				return false
			}
		}
	}
	if projectType == "mod" && loader != "" && loader != "vanilla" {
		if _, ok := version["loaders"]; ok {
			found := false
			for _, item := range stringList(version, "loaders") {
				if item == loader {
					found = true
					break
				}
			}
			if !found {
				return false
			}
		}
	}
	return true
}

func versionID(version map[string]any) string {
	if id := fieldString(version, "id"); id != "" {
		return id
	}
	return fieldString(version, "version_id")
}

func cmdSearch(req request) (any, error) {
	if err := validateProjectType(req.ProjectType); err != nil {
		return nil, err
	}
	sortBy := strings.ToLower(strings.TrimSpace(req.SortBy))
	if !searchOrderings[sortBy] {
		return nil, fmt.Errorf("Неизвестный порядок сортировки Modrinth: %s", sortBy)
	}
	limit := req.Limit
	if limit < 1 {
		limit = 1
	}
	if limit > 100 {
		limit = 100
	}
	loader := strings.ToLower(strings.TrimSpace(req.Loader))
	facets := [][]string{{"project_type:" + req.ProjectType}}
	if req.GameVersion != "" {
		facets = append(facets, []string{"versions:" + req.GameVersion})
	}
	if loader != "" && loader != "vanilla" && req.ProjectType == "mod" {
		facets = append(facets, []string{"categories:" + loader})
	}
	params := url.Values{}
	params.Set("limit", strconv.Itoa(limit))
	params.Set("index", sortBy)
	if encoded, err := json.Marshal(facets); err == nil {
		params.Set("facets", string(encoded))
	}
	offset := req.Offset
	if offset < 0 {
		offset = 0
	}
	if offset > 10000 {
		offset = 10000
	}
	if offset != 0 {
		params.Set("offset", strconv.Itoa(offset))
	}
	if trimmed := strings.TrimSpace(req.Query); trimmed != "" {
		params.Set("query", trimmed)
	}
	value, err := getJSON(httpClient(), baseURL(req.APIURL)+"/search?"+params.Encode())
	if err != nil {
		return nil, err
	}
	object := asObject(value)
	hits := []any{}
	if object != nil {
		if raw, ok := object["hits"].([]any); ok {
			for _, item := range raw {
				if entry := asObject(item); entry != nil && matchesProjectType(entry, req.ProjectType) {
					hits = append(hits, entry)
				}
			}
		}
	}
	total := any(nil)
	if object != nil {
		if number, ok := object["total_hits"].(float64); ok && number >= 0 {
			total = int(number)
		}
	}
	return map[string]any{"hits": hits, "total": total}, nil
}

func cmdDetails(req request) (any, error) {
	if err := validateProjectType(req.ProjectType); err != nil {
		return nil, err
	}
	projectID, err := validateProjectID(req.ProjectID)
	if err != nil {
		return nil, err
	}
	loader := strings.ToLower(strings.TrimSpace(req.Loader))
	client := httpClient()
	detailsRaw, err := getJSON(client, baseURL(req.APIURL)+"/project/"+url.PathEscape(projectID))
	if err != nil {
		return nil, err
	}
	details := asObject(detailsRaw)
	if details == nil {
		return nil, fmt.Errorf("Modrinth вернул некорректные данные проекта %s", projectID)
	}
	if actual := fieldString(details, "project_type"); actual != "" && actual != req.ProjectType {
		return nil, fmt.Errorf("Modrinth-проект %s имеет тип %s, ожидался %s", projectID, actual, req.ProjectType)
	}
	versionsRaw, err := getJSON(client, versionsURL(baseURL(req.APIURL), projectID, req.GameVersion, loader, req.ProjectType))
	if err != nil {
		return nil, err
	}
	return map[string]any{"details": details, "versions": filteredVersions(versionsRaw, projectID, req.GameVersion, loader, req.ProjectType)}, nil
}

func cmdVersions(req request) (any, error) {
	if err := validateProjectType(req.ProjectType); err != nil {
		return nil, err
	}
	projectID, err := validateProjectID(req.ProjectID)
	if err != nil {
		return nil, err
	}
	loader := strings.ToLower(strings.TrimSpace(req.Loader))
	versionsRaw, err := getJSON(httpClient(), versionsURL(baseURL(req.APIURL), projectID, req.GameVersion, loader, req.ProjectType))
	if err != nil {
		return nil, err
	}
	return filteredVersions(versionsRaw, projectID, req.GameVersion, loader, req.ProjectType), nil
}

func filteredVersions(raw any, projectID, gameVersion, loader, projectType string) []any {
	versions := []any{}
	list, _ := raw.([]any)
	for _, item := range list {
		version := asObject(item)
		if version == nil || !matchesProjectType(version, projectType) {
			continue
		}
		if !versionIsCompatible(version, gameVersion, loader, projectType) {
			continue
		}
		if reported := fieldString(version, "project_id"); reported != "" && reported != projectID {
			continue
		}
		versions = append(versions, version)
	}
	return versions
}

func versionsURL(base, projectID, gameVersion, loader, projectType string) string {
	params := url.Values{}
	if gameVersion != "" {
		if encoded, err := json.Marshal([]string{gameVersion}); err == nil {
			params.Set("game_versions", string(encoded))
		}
	}
	if loader != "" && loader != "vanilla" && projectType == "mod" {
		if encoded, err := json.Marshal([]string{loader}); err == nil {
			params.Set("loaders", string(encoded))
		}
	}
	target := base + "/project/" + url.PathEscape(projectID) + "/version"
	if len(params) == 0 {
		return target
	}
	return target + "?" + params.Encode()
}

type checkOutcome struct {
	ProjectID string `json:"project_id"`
	TypeOK    bool   `json:"type_ok"`
	Latest    any    `json:"latest"`
	Error     string `json:"error"`
}

func cmdCheck(req request) (any, error) {
	if err := validateProjectType(req.ProjectType); err != nil {
		return nil, err
	}
	loader := strings.ToLower(strings.TrimSpace(req.Loader))
	if len(req.Projects) == 0 {
		return []checkOutcome{}, nil
	}
	workers := req.Workers
	if workers < 1 {
		workers = maxBatchJobs
	}
	if workers > 32 {
		workers = 32
	}
	if workers > len(req.Projects) {
		workers = len(req.Projects)
	}
	jobs := make(chan int)
	results := make([]checkOutcome, len(req.Projects))
	var wg sync.WaitGroup
	wg.Add(workers)
	for i := 0; i < workers; i++ {
		go func() {
			defer wg.Done()
			for index := range jobs {
				results[index] = checkProject(httpClient(), req, req.Projects[index], loader)
			}
		}()
	}
	for index := range req.Projects {
		jobs <- index
	}
	close(jobs)
	wg.Wait()
	return results, nil
}

func checkProject(client *http.Client, req request, entry checkEntry, loader string) checkOutcome {
	outcome := checkOutcome{ProjectID: strings.TrimSpace(entry.ProjectID), TypeOK: true}
	projectID, err := validateProjectID(entry.ProjectID)
	if err != nil {
		outcome.Error = "Некорректный project ID ресурса"
		return outcome
	}
	detailsRaw, err := getJSON(client, baseURL(req.APIURL)+"/project/"+url.PathEscape(projectID))
	if err != nil {
		outcome.Error = err.Error()
		return outcome
	}
	details := asObject(detailsRaw)
	if details == nil {
		outcome.Error = fmt.Sprintf("Modrinth вернул некорректные данные проекта %s", projectID)
		return outcome
	}
	if actual := fieldString(details, "project_type"); actual != req.ProjectType {
		outcome.TypeOK = false
		return outcome
	}
	versionsRaw, err := getJSON(client, versionsURL(baseURL(req.APIURL), projectID, req.GameVersion, loader, req.ProjectType))
	if err != nil {
		outcome.Error = err.Error()
		return outcome
	}
	versions := filteredVersions(versionsRaw, projectID, req.GameVersion, loader, req.ProjectType)
	if len(versions) > 0 {
		outcome.Latest = versions[0]
	}
	return outcome
}

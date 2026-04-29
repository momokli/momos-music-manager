// tags-from-playlists.js - Wizard for creating tags from playlists
document.addEventListener("DOMContentLoaded", function () {
  // DOM elements
  const errorMessage = document.getElementById("error-message");
  const loadingState = document.getElementById("loading-state");
  const contentArea = document.getElementById("content-area");
  const playlistCount = document.getElementById("playlist-count");
  const playlistList = document.getElementById("playlist-list");
  const emptyState = document.getElementById("empty-state");
  const actionBar = document.getElementById("action-bar");
  const btnCreateTags = document.getElementById("btn-create-tags");
  const resultBox = document.getElementById("result-box");

  // State
  let playlists = [];

  // API endpoint
  const API_BASE = "http://localhost:3000/api";

  // Show error message
  function showError(message) {
    errorMessage.textContent = message;
    errorMessage.style.display = "block";
    setTimeout(function () {
      errorMessage.style.display = "none";
    }, 5000);
  }

  // Hide loading, show content
  function showContent() {
    loadingState.classList.add("hidden");
    contentArea.classList.remove("hidden");
  }

  // Escape HTML to prevent XSS
  function escapeHtml(text) {
    if (!text) return "";
    var div = document.createElement("div");
    div.appendChild(document.createTextNode(text));
    return div.innerHTML;
  }

  // Get service icon class based on service name
  function getServiceIcon(service) {
    switch (service) {
      case "spotify":
        return "fab fa-spotify";
      case "soundcloud":
        return "fab fa-soundcloud";
      case "youtube":
        return "fab fa-youtube";
      default:
        return "fas fa-music";
    }
  }

  // Render playlist list
  function renderPlaylists(playlistData) {
    // Update count
    var count = playlistData.length;
    playlistCount.textContent = count;

    // Clear previous list
    playlistList.innerHTML = "";

    if (count === 0) {
      // Show empty state
      playlistList.classList.add("hidden");
      emptyState.classList.remove("hidden");
      actionBar.classList.add("hidden");
      return;
    }

    // Show list, hide empty state
    playlistList.classList.remove("hidden");
    emptyState.classList.add("hidden");
    actionBar.classList.remove("hidden");

    // Sort playlists by name
    var sorted = playlistData.slice().sort(function (a, b) {
      return (a.name || "").localeCompare(b.name || "");
    });

    // Render each playlist item
    sorted.forEach(function (playlist) {
      var item = document.createElement("div");
      item.className = "playlist-item";

      // Name span
      var nameSpan = document.createElement("span");
      nameSpan.className = "playlist-name";
      nameSpan.textContent = playlist.name || "Untitled";

      // Service badge
      var badge = document.createElement("span");
      var service = (playlist.service || "unknown").toLowerCase();
      badge.className = "service-badge " + service;

      var icon = document.createElement("i");
      icon.className = getServiceIcon(service);
      badge.appendChild(icon);

      var serviceText = document.createTextNode(" " + service);
      badge.appendChild(serviceText);

      // Append to item
      item.appendChild(nameSpan);
      item.appendChild(badge);

      playlistList.appendChild(item);
    });
  }

  // Show result message
  function showResult(type, message) {
    resultBox.className = "result-box " + type;
    resultBox.innerHTML = message;
    resultBox.style.display = "block";
  }

  // Hide result
  function hideResult() {
    resultBox.className = "result-box";
    resultBox.style.display = "none";
  }

  // Fetch playlists without tags
  async function fetchPlaylistsWithoutTags() {
    try {
      var response = await fetch(API_BASE + "/tags/from-playlists");
      if (!response.ok) {
        throw new Error("HTTP error! status: " + response.status);
      }
      var result = await response.json();
      return result.data || { playlists: [], count: 0 };
    } catch (error) {
      console.error("Error fetching playlists without tags:", error);
      showError("Failed to load playlists: " + error.message);
      return { playlists: [], count: 0 };
    }
  }

  // Create tags from playlists
  async function createTagsFromPlaylists() {
    try {
      var response = await fetch(API_BASE + "/tags/create-from-playlists", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
      });
      if (!response.ok) {
        var errorData = null;
        try {
          errorData = await response.json();
        } catch (_) {
          // ignore parse error
        }
        var errorMsg =
          errorData && errorData.error
            ? errorData.error
            : "HTTP error! status: " + response.status;
        throw new Error(errorMsg);
      }
      var result = await response.json();
      return result.data || { created: 0, message: "No tags created" };
    } catch (error) {
      console.error("Error creating tags:", error);
      throw error;
    }
  }

  // Load playlists
  async function loadPlaylists() {
    // Show loading state
    loadingState.classList.remove("hidden");
    contentArea.classList.add("hidden");
    hideResult();

    // Fetch data
    var data = await fetchPlaylistsWithoutTags();
    playlists = data.playlists || [];

    // Update UI
    renderPlaylists(playlists);
    showContent();
  }

  // Handle create tags button click
  async function handleCreateTags() {
    // Disable button
    btnCreateTags.disabled = true;
    btnCreateTags.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Creating...';
    hideResult();

    try {
      var result = await createTagsFromPlaylists();
      var created = result.created || 0;

      // Show success
      showResult(
        "success",
        '<i class="fas fa-check-circle"></i> ' + escapeHtml(result.message) + "!",
      );

      // Reload list after 1 second
      setTimeout(function () {
        loadPlaylists();
      }, 1000);
    } catch (error) {
      // Show error
      showResult(
        "error",
        '<i class="fas fa-exclamation-circle"></i> Failed to create tags: ' +
          escapeHtml(error.message),
      );

      // Re-enable button
      btnCreateTags.disabled = false;
      btnCreateTags.innerHTML = '<i class="fas fa-wand-magic-sparkles"></i> Create Tags';
    }
  }

  // Event listeners
  btnCreateTags.addEventListener("click", handleCreateTags);

  // Initial load
  loadPlaylists();
});
